//! Validated economy data and deterministic M9 starting-state primitives.
//!
//! This crate is independent of maps, windows, Lua and renderers. The application
//! supplies authored scenario data and derived land counts through plain integers.

#![deny(unsafe_code)]

use std::{collections::BTreeMap, error::Error, fmt, fs, path::Path};

use ck_sim::{Canonical, CanonicalError, Date, Reader, encode_length};
use serde::Deserialize;

const WAGE_HISTORY_MONTHS: usize = 12;

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
    /// Human-readable unit.
    pub unit: String,
    /// Weight in grams per unit.
    pub weight_g: u64,
    /// Cargo volume in millilitres per unit (zero for live herds).
    pub volume_ml: u64,
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
}

/// Data describing one production method available to a building.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Method {
    /// Stable key.
    pub id: String,
    /// Maximum levels allowed by the resource this method consumes.
    pub resource: String,
    /// Annual output by good at suitability 1000.
    pub annual_output: BTreeMap<String, u64>,
    /// Per-good suitability category, normally terrain or sea.
    pub suitability: String,
    /// Output share in ‰ used to derive terrain land scores.
    pub score_share_pm: BTreeMap<String, u16>,
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
    /// Number of positions per owner, by population type.
    pub owner_workers: BTreeMap<String, u16>,
    /// Output penalty when owner coverage is incomplete, in ‰.
    pub penalty_pm: u16,
    /// Value retained for owners after the annual wage pool, in per-mille.
    pub overhead_pm: u16,
    /// Ordered methods; their order resolves equal estimates.
    #[serde(default)]
    pub methods: Vec<Method>,
}

/// An authored economy rules file.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Goods in canonical authoring order.
    pub goods: Vec<Good>,
    /// Population types.
    pub pop_types: Vec<PopType>,
    /// Starting necessities, niceties and luxuries by population type, in per-mille.
    #[serde(default)]
    pub starting_needs_pm: BTreeMap<String, [u16; 3]>,
    /// Cultures present in the simulation.
    pub cultures: Vec<String>,
    /// Religions present in the simulation.
    pub religions: Vec<String>,
    /// Land terrain names in the same order as the map terrain palette.
    pub terrain_order: Vec<String>,
    /// Footprint per building level, in square metres.
    pub footprint_m2: BTreeMap<String, i64>,
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
    /// Target-price response in per-mille.
    pub price_k_pm: u32,
}

impl Rules {
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
        let rules: Self = toml::from_str(&text).map_err(|error| DataError {
            file: file.clone(),
            entry: "<document>".to_owned(),
            problem: error.message().to_owned(),
        })?;
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
        unique_ids(
            file,
            "goods",
            self.goods.iter().map(|item| item.id.as_str()),
        )?;
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
        unique_ids(file, "cultures", self.cultures.iter().map(String::as_str))?;
        unique_ids(file, "religions", self.religions.iter().map(String::as_str))?;
        unique_ids(
            file,
            "terrain_order",
            self.terrain_order.iter().map(String::as_str),
        )?;
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
                if !["usable_land", "sea"].contains(&method.resource.as_str()) {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}.resource", method.id),
                        "must be `usable_land` or `sea`".to_owned(),
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
                if !["cattle", "pigs", "sheep"].contains(&animal.as_str()) {
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
    /// Culture for the monk population (Mayo is English).
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

/// Per-scenario authored economy starting data.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScenarioEconomy {
    /// Defaults indexed by location kind and decimal size.
    pub defaults: BTreeMap<String, BTreeMap<String, StartDefaults>>,
    /// Per-location exceptions.
    #[serde(default)]
    pub places: BTreeMap<String, StartOverride>,
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

/// Plain, derived cell counts supplied by the map adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LandCounts {
    /// All owned land cells by terrain key, excluding glaciers.
    pub land_cells: BTreeMap<String, u32>,
    /// Sum of fertility per-mille by terrain key, excluding glaciers.
    pub fertility_pm: BTreeMap<String, u64>,
    /// Owned land cells whose fertility is above zero, excluding glaciers.
    pub fertile_cells: u32,
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

/// Build the deterministic opening economy from authored scenario data.
///
/// The report names every resource cut and any authored level for which no
/// method had room. Employment is filled directly and has no monthly rate cap.
///
/// # Errors
/// Returns a source-qualified error for invalid rules, missing templates or checked arithmetic overflow.
#[expect(
    clippy::too_many_lines,
    reason = "The per-location startup sequence is kept together so holdings, farm, storehouse, and employment ordering stays explicit."
)]
pub fn starting_state(
    rules: &Rules,
    data: &ScenarioEconomy,
    locations: &[StartLocation],
    start_month: u8,
) -> Result<(EconomyState, Vec<String>), DataError> {
    if !(1..=12).contains(&start_month) {
        return Err(DataError {
            file: "scenario start".to_owned(),
            entry: "start_month".to_owned(),
            problem: "must be in 1..=12".to_owned(),
        });
    }
    rules.validate("economy rules")?;
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
        for (field, value, choices) in [
            ("culture", culture, &rules.cultures),
            ("religion", religion, &rules.religions),
            ("monk_culture", monk_culture, &rules.cultures),
            ("monk_religion", monk_religion, &rules.religions),
        ] {
            if !choices.iter().any(|choice| choice == value) {
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
            if !rules
                .cultures
                .iter()
                .any(|choice| choice == peasant_culture)
            {
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
        let mut land = location.land.clone();
        let mut buildings = Vec::new();
        let mut remaining_sea = land.sea;
        let mut production = BTreeMap::<String, u64>::new();
        // Holdings take fertility before the farm. Fishing needs nearby sea but
        // does not consume a finite sea-cell budget.
        for (slot, kind) in [(1_usize, "holdings"), (0_usize, "farm")] {
            let wanted = authored[slot];
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
                &production,
                &pops,
                location.land.sea > 0,
                mean_fertility(&location.land)?,
            )?;
            let actual = assignment.method_levels.iter().sum();
            if actual < wanted {
                report.push(format!(
                    "{} {kind}: cut levels {wanted} -> {actual} (no method has enough land or sea)",
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
            let herds = herd_counts(&assignment.method_levels, building);
            let herd_carry = vec![0; herds.len()];
            buildings.push(BuildingState {
                kind: kind.to_owned(),
                levels: actual,
                method_levels: assignment.method_levels,
                herds,
                employment: vec![Employment::default(); pops.len()],
                stock: stock_vector(rules, &defaults.building_stock),
                storehouse_accounts: None,
                production_carries: vec![vec![0; rules.goods.len()]; building.methods.len()],
                method_suitability: assignment.method_suitability,
                herd_grazing_sums: assignment.herd_grazing_sums,
                herd_birth_carries: herd_carry.clone(),
                herd_death_carries: herd_carry,
                herd_product_carries: vec![0; rules.goods.len()],
                wage_carries: vec![0; rules.goods.len()],
                pay_carries: vec![0; rules.goods.len()],
                fee_carries: Vec::new(),
                wage_history: vec![0; 12],
            });
            let _ = (assignment.used_land, assignment.used_sea);
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
                herd_grazing_sums: Vec::new(),
            });
        }
        fill_starting_employment(rules, &mut pops, &mut buildings);
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
            coastal: location.land.sea > 0,
            pops,
            buildings,
            prices_milli,
            prepared_m2,
            development_pm: rules.starting_development_pm,
            price_drift_carries: vec![0; rules.goods.len()],
            counters: CounterBuckets::default(),
        });
    }
    Ok((state, report))
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
    method_suitability: Vec<Vec<u64>>,
    herd_grazing_sums: Vec<u64>,
    used_land: u32,
    used_sea: u32,
    output: BTreeMap<String, u64>,
}

#[expect(
    clippy::too_many_lines,
    reason = "Each level must compare all currently eligible methods before mutating the shared land, sea, and supply budgets."
)]
#[expect(
    clippy::too_many_arguments,
    reason = "These references are the shared per-location startup budgets and fixed sell-side inputs for one sequential building assignment."
)]
fn assign_building_levels(
    rules: &Rules,
    building: &BuildingType,
    wanted: u32,
    land: &mut LandCounts,
    remaining_sea: &mut u32,
    prior_sell: &BTreeMap<String, u64>,
    pops: &[PopState],
    coastal: bool,
    quality_pm: u16,
) -> Result<BuildingAssignment, DataError> {
    let mut assigned = vec![0_u32; building.methods.len()];
    let mut method_suitability = vec![vec![0_u64; rules.goods.len()]; building.methods.len()];
    let mut herd_grazing_sums = vec![0_u64; 3];
    let mut used_land = 0_u32;
    let mut used_sea = 0_u32;
    let mut output = BTreeMap::<String, u64>::new();
    for _ in 0..wanted {
        let mut best: Option<(usize, u128, Option<String>)> = None;
        for (index, method) in building.methods.iter().enumerate() {
            let terrain = if method.resource == "sea" {
                if *remaining_sea == 0 {
                    continue;
                }
                None
            } else {
                choose_terrain(
                    rules,
                    method,
                    land,
                    u64::from(building.land_per_level) * 1000,
                )
            };
            if method.resource != "sea" && terrain.is_none() {
                continue;
            }
            let mut estimate = 0_u128;
            for (good_id, base_output) in &method.annual_output {
                let good = rules
                    .goods
                    .iter()
                    .find(|good| &good.id == good_id)
                    .ok_or_else(|| {
                        missing_data(
                            format!("goods.{good_id}"),
                            "method output references unknown good",
                        )
                    })?;
                let output_units = starting_method_output(
                    rules,
                    method,
                    good_id,
                    *base_output,
                    terrain.as_deref(),
                    quality_pm,
                )?;
                let sell_before = u128::from(prior_sell.get(good_id).copied().unwrap_or(0))
                    .checked_add(u128::from(output.get(good_id).copied().unwrap_or(0)))
                    .ok_or_else(|| missing_data("projection".to_owned(), "supply overflow"))?;
                let buy_units = projected_buy_units(rules, pops, good_id, coastal)?;
                let base_milli = good.base_price.checked_mul(1000).ok_or_else(|| {
                    missing_data(
                        format!("goods.{}.base_price", good.id),
                        "base price in thousandths overflows u64",
                    )
                })?;
                let buy_value = value_at_price(buy_units, base_milli)
                    .map_err(|problem| missing_data("projection".to_owned(), problem))?;
                let sell_value = value_at_price(sell_before, base_milli)
                    .map_err(|problem| missing_data("projection".to_owned(), problem))?;
                let price = target_price(good.base_price, buy_value, sell_value, rules.price_k_pm)
                    .map_err(|problem| missing_data(format!("target_price.{good_id}"), problem))?;
                let output_value = value_at_price(output_units, price)
                    .map_err(|problem| missing_data("projection".to_owned(), problem))?;
                estimate = estimate.checked_add(output_value).ok_or_else(|| {
                    missing_data("projection".to_owned(), "method value sum overflow")
                })?;
            }
            estimate = estimate
                .checked_mul(u128::from(building.overhead_pm))
                .ok_or_else(|| {
                    missing_data("projection".to_owned(), "net method value overflow")
                })?
                / 1000;
            if best.as_ref().is_none_or(|(_, value, _)| estimate > *value) {
                best = Some((index, estimate, terrain));
            }
        }
        let Some((index, _, terrain)) = best else {
            break;
        };
        let method = &building.methods[index];
        assigned[index] += 1;
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
            let suitability = method_suitability_pm(rules, method, good_id, terrain.as_deref());
            method_suitability[index][good_index] = method_suitability[index][good_index]
                .checked_add(u64::from(suitability))
                .ok_or_else(|| missing_data("projection".to_owned(), "suitability sum overflow"))?;
        }
        if let Some(terrain) = terrain.as_deref() {
            for (species, animal) in ["cattle", "pigs", "sheep"].into_iter().enumerate() {
                let grazing = rules
                    .grazing
                    .get(terrain)
                    .and_then(|values| values.get(animal))
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
        if method.resource == "sea" {
            used_sea = used_sea.saturating_add(1);
        } else if let Some(terrain) = terrain.as_ref() {
            let fertility = land
                .fertility_pm
                .get_mut(terrain)
                .expect("selected terrain exists");
            let cost = u64::from(building.land_per_level) * 1000;
            *fertility -= cost;
            used_land = used_land.saturating_add(u32::from(building.land_per_level));
        }
        for (good_id, quantity) in &method.annual_output {
            let produced = starting_method_output(
                rules,
                method,
                good_id,
                *quantity,
                terrain.as_deref(),
                quality_pm,
            )?;
            let produced = u64::try_from(produced)
                .map_err(|_| missing_data("projection".to_owned(), "annual output exceeds u64"))?;
            let current = output.entry(good_id.clone()).or_default();
            *current = (*current).checked_add(produced).ok_or_else(|| {
                missing_data("projection".to_owned(), "annual output sum overflow")
            })?;
        }
    }
    Ok(BuildingAssignment {
        method_levels: assigned,
        method_suitability,
        herd_grazing_sums,
        used_land,
        used_sea,
        output,
    })
}

fn starting_method_output(
    rules: &Rules,
    method: &Method,
    good_id: &str,
    base_output: u64,
    terrain: Option<&str>,
    quality_pm: u16,
) -> Result<u128, DataError> {
    let suitability = method_suitability_pm(rules, method, good_id, terrain);
    let mut numerator = u128::from(base_output)
        .checked_mul(u128::from(suitability))
        .ok_or_else(|| {
            missing_data(
                format!("buildings.methods.{}.annual_output.{good_id}", method.id),
                "annual output estimate overflow",
            )
        })?;
    let mut denominator = 1000_u128;
    if method.resource != "sea" {
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
                owners = owners.checked_add(employment.owners).ok_or_else(|| {
                    missing_data("employment.owners".to_owned(), "owner count overflow")
                })?;
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
    reason = "The fee debit and payday credit share one transactional storehouse accounting step."
)]
#[expect(
    clippy::too_many_arguments,
    reason = "The current local prices and month index belong to the atomic fee payday step."
)]
fn pay_storehouse_fee(
    rules: &Rules,
    place_id: &str,
    prices_milli: &[u64],
    month_index: usize,
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
        // Value this payday's fees only; old pay carries are released below but are not new fees.
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

fn method_suitability_pm(
    rules: &Rules,
    method: &Method,
    good_id: &str,
    terrain: Option<&str>,
) -> u16 {
    let suitability = if method.resource == "sea" {
        1000_u128
    } else if ["beef", "pork", "mutton", "dairy", "wool", "hides"].contains(&good_id) {
        // Authored herd yields are per maximum herd; each starting herd is three quarters full.
        750
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

fn projected_buy_units(
    rules: &Rules,
    pops: &[PopState],
    good_id: &str,
    coastal: bool,
) -> Result<u128, DataError> {
    let good = rules
        .goods
        .iter()
        .find(|good| good.id == good_id)
        .ok_or_else(|| missing_data(format!("goods.{good_id}"), "definition is missing"))?;
    if good.hunger_centi == 0 {
        return Ok(0);
    }
    let mut total = 0_u128;
    for pop in pops {
        let shares = rules.diet_profiles.get(&pop.kind).ok_or_else(|| {
            missing_data(format!("diet_profiles.{}", pop.kind), "profile is missing")
        })?;
        let normalized = normalized_diet_shares(rules, shares, coastal)?;
        let share = rules
            .goods
            .iter()
            .position(|good| good.id == good_id)
            .and_then(|index| normalized.get(index).copied())
            .unwrap_or(0);
        if share == 0 {
            continue;
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
        total = total
            .checked_add(numerator / denominator)
            .ok_or_else(|| missing_data("projection".to_owned(), "diet demand sum overflow"))?;
    }
    Ok(total)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DailyMarketOrders {
    buy_units: Vec<u128>,
    sell_units: Vec<u128>,
}

fn food_target_units(
    rules: &Rules,
    pop_kind: &str,
    people: u64,
    good_index: usize,
    coastal: bool,
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
    let shares = normalized_diet_shares(rules, profile, coastal)?;
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
        .and_then(|target| target.checked_mul(240))
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

fn daily_market_orders(rules: &Rules, place: &PlaceState) -> Result<DailyMarketOrders, DataError> {
    let goods = rules.goods.len();
    let mut orders = DailyMarketOrders {
        buy_units: vec![0; goods],
        sell_units: vec![0; goods],
    };
    let mut pop_sizes = Vec::with_capacity(place.pops.len());
    for (pop_index, pop) in place.pops.iter().enumerate() {
        let people = pop.workers.checked_add(pop.dependents).ok_or_else(|| {
            missing_data("market.pop.size".to_owned(), "population size overflow")
        })?;
        pop_sizes.push(people);
        for (good_index, good) in rules.goods.iter().enumerate() {
            let holding = pop_market_holding(place, pop_index, good_index)?;
            let target =
                food_target_units(rules, &pop.kind, people, good_index, place.coastal, 240)?;
            let comfort_target = tier_target_units(rules, &pop.kind, people, good_index, 1)?
                .max(tier_target_units(rules, &pop.kind, people, good_index, 2)?);
            let target = target.max(comfort_target);
            let buy = target.saturating_sub(holding);
            orders.buy_units[good_index] = orders.buy_units[good_index]
                .checked_add(buy)
                .ok_or_else(|| missing_data("market.buy".to_owned(), "buy order overflow"))?;
            if good.hunger_centi > 0 {
                let sell = holding.saturating_sub(target);
                orders.sell_units[good_index] = orders.sell_units[good_index]
                    .checked_add(sell)
                    .ok_or_else(|| missing_data("market.sell".to_owned(), "sell order overflow"))?;
            }
        }
    }
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
    for (building_index, building) in place.buildings.iter().enumerate() {
        for (good_index, good) in rules.goods.iter().enumerate() {
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
                let target =
                    food_target_units(rules, &pop.kind, people, good_index, place.coastal, 60)?;
                owner_reserve = owner_reserve.checked_add(target).ok_or_else(|| {
                    missing_data("market.owner_reserve".to_owned(), "owner reserve overflow")
                })?;
            }
            if good.hunger_centi > 0 {
                let sell = holding.saturating_sub(owner_reserve);
                orders.sell_units[good_index] = orders.sell_units[good_index]
                    .checked_add(sell)
                    .ok_or_else(|| missing_data("market.sell".to_owned(), "sell order overflow"))?;
            } else {
                orders.sell_units[good_index] = orders.sell_units[good_index]
                    .checked_add(holding)
                    .ok_or_else(|| missing_data("market.sell".to_owned(), "sell order overflow"))?;
            }
        }
    }
    Ok(orders)
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
    }
    Ok(())
}

fn normalized_diet_shares(
    rules: &Rules,
    shares: &BTreeMap<String, u16>,
    coastal: bool,
) -> Result<Vec<u64>, DataError> {
    let weights = rules
        .goods
        .iter()
        .map(|good| {
            if !coastal && good.id == "fish" {
                0
            } else {
                u64::from(shares.get(&good.id).copied().unwrap_or(0))
            }
        })
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

fn herd_counts(method_levels: &[u32], building: &BuildingType) -> Vec<u64> {
    let farming_levels = building
        .methods
        .iter()
        .enumerate()
        .filter(|(_, method)| method.resource != "sea")
        .map(|(index, _)| method_levels[index])
        .sum::<u32>();
    vec![
        u64::from(farming_levels) * 9,
        u64::from(farming_levels) * 6,
        u64::from(farming_levels) * 5,
    ]
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
    for (building_index, building) in buildings.iter_mut().enumerate() {
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
        } else if building.kind == "storehouse" {
            for index in 0..pops.len() {
                let owns_a_served_building = pops[index]
                    .employment
                    .iter()
                    .take(building_index)
                    .any(|employment| employment.owners > 0);
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

/// Monthly births, deaths and overflow losses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonthlyCounters {
    /// People born.
    pub births: u64,
    /// People who died.
    pub deaths: u64,
    /// Whole goods units lost after payday overflow.
    pub losses: u64,
}

/// Display-only counters saved across a quicksave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CounterBuckets {
    /// Events accumulated since this month's first-of-month rollover.
    pub current: MonthlyCounters,
    /// Events from the month completed by the most recent rollover.
    pub last: MonthlyCounters,
}

fn roll_counter_month(counters: &mut CounterBuckets) {
    counters.last.clone_from(&counters.current);
    counters.current = MonthlyCounters::default();
}

fn encode_counters(counters: &CounterBuckets, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    for bucket in [&counters.current, &counters.last] {
        bucket.births.encode(out)?;
        bucket.deaths.encode(out)?;
        bucket.losses.encode(out)?;
    }
    Ok(())
}

fn decode_counters(input: &mut Reader<'_>) -> Result<CounterBuckets, CanonicalError> {
    let current = MonthlyCounters {
        births: u64::decode(input)?,
        deaths: u64::decode(input)?,
        losses: u64::decode(input)?,
    };
    let last = MonthlyCounters {
        births: u64::decode(input)?,
        deaths: u64::decode(input)?,
        losses: u64::decode(input)?,
    };
    Ok(CounterBuckets { current, last })
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
}

/// A building in the initial economy state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildingState {
    /// Building type key.
    pub kind: String,
    /// Total authored levels after resource cuts.
    pub levels: u32,
    /// Levels per method in method order.
    pub method_levels: Vec<u32>,
    /// Herd counts in species order.
    pub herds: Vec<u64>,
    /// Employment by pop index.
    pub employment: Vec<Employment>,
    /// Goods stock in rules order.
    pub stock: Vec<u64>,
    /// Storehouse claims in pop order plus the communal account.
    /// Present only for storehouses; inner vectors use goods order.
    pub storehouse_accounts: Option<StorehouseAccounts>,
    /// Fractional seasonal production carries, in method and good order.
    pub production_carries: Vec<Vec<u64>>,
    /// Sum of per-level product suitability in per-mille, by method and good.
    pub method_suitability: Vec<Vec<u64>>,
    /// Fractional herd births per species, in 1e12ths of an animal.
    pub herd_birth_carries: Vec<u64>,
    /// Fractional herd deaths per species, in 12,000ths of an animal.
    pub herd_death_carries: Vec<u64>,
    /// Fractional dairy and wool product carries in good order.
    pub herd_product_carries: Vec<u64>,
    /// Sum of per-level grazing suitability for cattle, pigs and sheep.
    pub herd_grazing_sums: Vec<u64>,
    /// Fractional wage shares in millionths, in good order.
    pub wage_carries: Vec<u64>,
    /// Whole wage units awaiting workers, in good order.
    pub pay_carries: Vec<u64>,
    /// Storehouse account fee carries in pop order, then communal account.
    pub fee_carries: Vec<Vec<u64>>,
    /// Posted wages in thousandths of a value unit per paid head, by calendar month.
    pub wage_history: Vec<u64>,
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
            if group.is_some_and(|wanted| storehouse_group(&good.id) != Some(wanted)) {
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

fn store_capacity_ml(building: &BuildingState) -> Result<u128, DataError> {
    let workers = u128::from(worker_count(building)?);
    let per_worker = match building.kind.as_str() {
        "farm" => 100_000,
        "holdings" => 1_170_000,
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
        "holdings" => 730,
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
    let by_volume = if good.volume_ml == 0 {
        offered
    } else {
        u64::try_from((volume_room / u128::from(good.volume_ml)).min(u128::from(u64::MAX)))
            .expect("bounded by u64::MAX")
    };
    Ok(offered.min(cap_room).min(by_volume))
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
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("pop.stock".to_owned(), "good index is outside rules"))?;
    let people = pop
        .workers
        .checked_add(pop.dependents)
        .ok_or_else(|| missing_data("pop.size".to_owned(), "population size overflow"))?;
    let good_cap = u64::try_from(u128::from(good.pop_limit_milli) * u128::from(people) / 1000)
        .map_err(|_| missing_data("pop.cap".to_owned(), "good cap exceeds u64"))?;
    let capacity = u128::from(people)
        .checked_mul(720_000)
        .ok_or_else(|| missing_data("pop.capacity".to_owned(), "volume capacity overflow"))?;
    let accepted = room_by_cap_and_volume(
        rules, &pop.stock, good_index, good_cap, capacity, None, offered,
    )?;
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
        if remaining > 0 {
            place_counters.current.losses = place_counters
                .current
                .losses
                .checked_add(remaining)
                .ok_or_else(|| {
                    missing_data("counters.losses".to_owned(), "place loss counter overflow")
                })?;
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
    /// Whether the location's catchment includes sea, which enables its authored fish diet share.
    pub coastal: bool,
    /// Pops in authored order.
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
    /// Display-only monthly event counters.
    pub counters: CounterBuckets,
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

fn curve_value(points: &[[i32; 2]], x: u64) -> Result<u64, DataError> {
    let x = i64::try_from(x)
        .map_err(|_| missing_data("curves.intake_food_days".to_owned(), "input exceeds i64"))?;
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
    fn new(good_count: usize) -> Self {
        Self {
            produced: vec![0; good_count],
            lost: vec![0; good_count],
            herd_births: vec![0; 3],
            herd_deaths: vec![0; 3],
            herd_slaughter: vec![0; 3],
            stock_before: vec![0; good_count],
            stock_after: vec![0; good_count],
            unpaid_wages: vec![0; good_count],
            intake_pm: Vec::new(),
            daily_consumption: None,
            herds_before: vec![0; 3],
            herds_after: vec![0; 3],
            places: Vec::new(),
        }
    }
}

impl PlaceProductionLedger {
    fn new(place_id: &str, good_count: usize) -> Self {
        Self {
            place_id: place_id.to_owned(),
            produced: vec![0; good_count],
            lost: vec![0; good_count],
            herd_births: vec![0; 3],
            herd_deaths: vec![0; 3],
            herd_slaughter: vec![0; 3],
            stock_before: vec![0; good_count],
            stock_after: vec![0; good_count],
            unpaid_wages: vec![0; good_count],
            herds_before: vec![0; 3],
            herds_after: vec![0; 3],
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

fn total_state_herds(state: &EconomyState) -> Result<Vec<u64>, DataError> {
    let mut totals = vec![0_u64; 3];
    for place in &state.places {
        for (total, amount) in totals.iter_mut().zip(total_herds(place)?) {
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

fn total_herds(place: &PlaceState) -> Result<Vec<u64>, DataError> {
    let mut totals = vec![0_u64; 3];
    for building in &place.buildings {
        if !building.herds.is_empty() && building.herds.len() != totals.len() {
            return Err(missing_data(
                format!("places.{}.buildings.{}.herds", place.id, building.kind),
                "saved herd dimensions must contain cattle, pigs and sheep",
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
    let species_order = [1_usize, 2, 0];
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
            .filter(|(method, _)| method.resource == "usable_land")
            .map(|(_, levels)| *levels)
            .sum::<u32>();
        for species in species_order {
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
                let (meat_id, yield_per_head) = match species {
                    0 => ("beef", 220_u64),
                    1 => ("pork", 88),
                    _ => ("mutton", 33),
                };
                let meat_index = index_for_good(rules, meat_id)?;
                let before = held_hunger(rules, &place.pops[pop_index])?;
                place.buildings[building_index].herds[species] -= 1;
                add_daily_good(
                    &mut ledger.herd_slaughter,
                    species,
                    1,
                    "ledger.daily.herd_slaughter",
                )?;
                add_daily_good(
                    &mut ledger.butchered,
                    meat_index,
                    yield_per_head,
                    "ledger.daily.butchered",
                )?;
                let left = receive_pop(
                    rules,
                    &mut place.pops[pop_index],
                    meat_index,
                    yield_per_head,
                )?;
                let lost = left;
                if lost != 0 {
                    add_daily_good(&mut ledger.lost, meat_index, lost, "ledger.daily.lost")?;
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
                if species == 0 {
                    let hides_index = index_for_good(rules, "hides")?;
                    add_daily_good(
                        &mut ledger.butchered,
                        hides_index,
                        1,
                        "ledger.daily.butchered",
                    )?;
                    let hide_left = receive_pop(rules, &mut place.pops[pop_index], hides_index, 1)?;
                    if hide_left != 0 {
                        add_daily_good(
                            &mut ledger.lost,
                            hides_index,
                            hide_left,
                            "ledger.daily.lost",
                        )?;
                        place.pops[pop_index].counters.current.losses = place.pops[pop_index]
                            .counters
                            .current
                            .losses
                            .checked_add(hide_left)
                            .ok_or_else(|| {
                                missing_data("butchering.loss".to_owned(), "loss counter overflow")
                            })?;
                        place.counters.current.losses = place
                            .counters
                            .current
                            .losses
                            .checked_add(hide_left)
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

fn run_daily_consumption_with_ledger(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<(Vec<Vec<u16>>, DailyConsumptionLedger), DataError> {
    rules.validate("economy rules")?;
    let mut next = state.clone();
    let mut ledger = DailyConsumptionLedger {
        stock_before: total_state_stored_goods(rules, &next)?,
        herds_before: total_state_herds(&next)?,
        butchered: vec![0; rules.goods.len()],
        opened: vec![0; rules.goods.len()],
        ticked: vec![0; rules.goods.len()],
        lost: vec![0; rules.goods.len()],
        herd_slaughter: vec![0; 3],
        ..DailyConsumptionLedger::default()
    };
    let pop_types = &rules.pop_types;
    let mut intake_by_place = Vec::with_capacity(next.places.len());
    for place in &mut next.places {
        let mut place_intake = Vec::with_capacity(place.pops.len());
        let mut index = 0;
        while index < place.pops.len() {
            draw_pop(rules, place, index)?;
            let intake_pm = consume_pop_tracked(rules, place, index, &mut ledger)?;
            place_intake.push(intake_pm);
            if date.day == 1 {
                let pop_type = pop_types
                    .iter()
                    .find(|item| item.id == place.pops[index].kind)
                    .ok_or_else(|| {
                        missing_data(
                            format!("pop_types.{}", place.pops[index].kind),
                            "definition is missing",
                        )
                    })?;
                let consumption = &mut place.pops[index].consumption;
                let target = satisfaction_baseline(
                    pop_type,
                    [
                        consumption.necessities_pm,
                        consumption.niceties_pm,
                        consumption.luxuries_pm,
                    ],
                );
                consumption.satisfaction = i16::try_from(drift_value(
                    i64::from(consumption.satisfaction),
                    target,
                    100,
                    1000,
                    &mut consumption.drift_carries[3],
                    -100,
                    100,
                )?)
                .map_err(|_| missing_data("satisfaction".to_owned(), "value exceeds i16"))?;
            }
            index += 1;
        }
        intake_by_place.push(place_intake);
    }
    ledger.stock_after = total_state_stored_goods(rules, &next)?;
    ledger.herds_after = total_state_herds(&next)?;
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
        .any(|(pantry, good)| *pantry >= u64::from(good.hunger_centi) && good.hunger_centi != 0)
        || consumption
            .pantry_hunger_centi
            .iter()
            .zip(&rules.goods)
            .any(|(pantry, good)| *pantry != 0 && good.hunger_centi == 0)
    {
        return Err(missing_data(
            "pop.consumption.pantry_hunger_centi".to_owned(),
            "saved hunger pantry is outside its per-good remainder range",
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

/// Run the first-of-month seasonal production phase, committing all places together.
///
/// Production uses the method assignment and terrain suitability saved at startup.
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
#[expect(
    clippy::too_many_lines,
    reason = "The first-of-month order keeps production, herd updates, payday and storehouse accounting in one rollback boundary."
)]
pub fn run_monthly_production_with_quality(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
) -> Result<ProductionLedger, DataError> {
    if date.day != 1 {
        return Ok(ProductionLedger::new(rules.goods.len()));
    }
    if !(1..=12).contains(&date.month) {
        return Err(missing_data(
            "calendar.month".to_owned(),
            "month is outside 1..=12",
        ));
    }
    rules.validate("economy rules")?;
    let mut next = state.clone();
    let mut ledger = ProductionLedger::new(rules.goods.len());
    for place in &mut next.places {
        let quality_pm = quality_by_place
            .get(&place.id)
            .copied()
            .unwrap_or(rules.fertility_reference_pm);
        roll_counter_month(&mut place.counters);
        for pop in &mut place.pops {
            roll_counter_month(&mut pop.counters);
        }
        let mut place_ledger = PlaceProductionLedger::new(&place.id, rules.goods.len());
        place_ledger.stock_before = total_stored_goods(rules, place)?;
        place_ledger.herds_before = total_herds(place)?;
        let mut pending_overflow = Vec::<(usize, usize, Vec<(Option<usize>, u64)>)>::new();
        let mut fee_buildings = Vec::new();
        for (building_index, building) in place.buildings.iter_mut().enumerate() {
            let Some(definition) = rules.buildings.iter().find(|item| item.id == building.kind)
            else {
                return Err(missing_data(
                    format!("buildings.{}", building.kind),
                    "definition is missing",
                ));
            };
            if building.wage_history.len() != WAGE_HISTORY_MONTHS {
                return Err(missing_data(
                    format!(
                        "places.{}.buildings.{}.wage_history",
                        place.id, building.kind
                    ),
                    "saved wage history must have exactly 12 calendar months",
                ));
            }
            if definition.methods.is_empty() {
                if building.storehouse_accounts.is_some() {
                    fee_buildings.push(building_index);
                }
                continue;
            }
            if definition.methods.len() != building.method_levels.len()
                || building.production_carries.len() != definition.methods.len()
                || building.method_suitability.len() != definition.methods.len()
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
            let mut remaining_workers = workers.min(positions);
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
                let levels = u64::from(building.method_levels[method_index]);
                if levels == 0 {
                    continue;
                }
                let method_positions = levels
                    .checked_mul(u64::from(definition.workers_per_level))
                    .ok_or_else(|| {
                        missing_data(
                            "employment.positions".to_owned(),
                            "method position count overflow",
                        )
                    })?;
                let method_staffed = remaining_workers.min(method_positions);
                remaining_workers -= method_staffed;
                for (good_id, annual) in &method.annual_output {
                    if ["beef", "pork", "mutton", "dairy", "wool", "hides"]
                        .contains(&good_id.as_str())
                    {
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
                    let suitability =
                        u128::from(building.method_suitability[method_index][good_index]);
                    let farming = method.resource != "sea";
                    let (quality_factor, reference_factor) = if farming {
                        reduced_ratio(quality_pm, rules.fertility_reference_pm)
                    } else {
                        (1, 1)
                    };
                    let numerator = u128::from(*annual)
                        .checked_mul(suitability)
                        .and_then(|value| value.checked_mul(month_share))
                        .and_then(|value| value.checked_mul(u128::from(method_staffed)))
                        .and_then(|value| value.checked_mul(u128::from(coverage_efficiency)))
                        .and_then(|value| value.checked_mul(u128::from(quality_factor)))
                        .and_then(|value| {
                            value.checked_add(u128::from(
                                building.production_carries[method_index][good_index],
                            ))
                        })
                        .ok_or_else(|| {
                            missing_data(
                                format!(
                                    "places.{}.buildings.{}.production.{good_id}",
                                    place.id, building.kind
                                ),
                                "monthly output numerator overflow",
                            )
                        })?;
                    let denominator = u128::from(method_positions)
                        .checked_mul(1_000_000_000)
                        .and_then(|value| value.checked_mul(u128::from(reference_factor)))
                        .ok_or_else(|| {
                            missing_data(
                                "production.denominator".to_owned(),
                                "production denominator overflow",
                            )
                        })?;
                    let output = u64::try_from(numerator / denominator).map_err(|_| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.production.{good_id}",
                                place.id, building.kind
                            ),
                            "monthly output exceeds u64",
                        )
                    })?;
                    building.production_carries[method_index][good_index] =
                        u64::try_from(numerator % denominator).map_err(|_| {
                            missing_data(
                                "production.carry".to_owned(),
                                "production carry exceeds u64",
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
                                "monthly stock overflow",
                            )
                        })?;
                }
            }
            if !building.herds.is_empty() {
                if building.herds.len() != 3
                    || building.herd_birth_carries.len() != 3
                    || building.herd_death_carries.len() != 3
                    || building.herd_product_carries.len() != rules.goods.len()
                    || building.herd_grazing_sums.len() != 3
                {
                    return Err(missing_data(
                        format!("places.{}.buildings.{}.herds", place.id, building.kind),
                        "herd and carry dimensions must contain cattle, pigs and sheep",
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
                let birth_rates = [500_u64, 1_500, 800];
                let death_rates = [80_u64, 100, 80];
                let maximum_per_level = [12_u64, 8, 7];
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
                for species in 0..3 {
                    let herd = building.herds[species];
                    let birth_num = u128::from(herd)
                        .checked_mul(u128::from(birth_rates[species]))
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
                        .checked_mul(u128::from(death_rates[species]))
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
                        .checked_mul(maximum_per_level[species])
                        .ok_or_else(|| missing_data("herds".to_owned(), "herd maximum overflow"))?;
                    let slaughtered = surviving.saturating_sub(maximum);
                    place_ledger.herd_slaughter[species] = place_ledger.herd_slaughter[species]
                        .checked_add(slaughtered)
                        .ok_or_else(|| {
                            missing_data("ledger.herds".to_owned(), "slaughter total overflow")
                        })?;
                    building.herds[species] = surviving - slaughtered;
                    let yields: &[(&str, u64)] = match species {
                        0 => &[("beef", 220), ("hides", 1)],
                        1 => &[("pork", 88)],
                        _ => &[("mutton", 33)],
                    };
                    for (good_id, yield_per_head) in yields {
                        let Some(good_index) =
                            rules.goods.iter().position(|good| good.id == *good_id)
                        else {
                            continue;
                        };
                        let output = slaughtered.checked_mul(*yield_per_head).ok_or_else(|| {
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
                for (good_id, base, season_key, denominator) in [
                    ("dairy", 72_u64, Some("dairy"), 1_000_000_u64),
                    ("wool", 15_u64, None, 10_000_u64),
                ] {
                    let Some(good_index) = rules.goods.iter().position(|good| good.id == good_id)
                    else {
                        continue;
                    };
                    let eligible_month = season_key.is_some() || date.month == 6;
                    if !eligible_month {
                        continue;
                    }
                    let season_share = if let Some(key) = season_key {
                        let shares = rules.seasons.get(key).ok_or_else(|| {
                            missing_data(format!("seasons.{key}"), "season table is missing")
                        })?;
                        u128::from(
                            *shares
                                .get(usize::from(date.month.saturating_sub(1)))
                                .ok_or_else(|| {
                                    missing_data(
                                        format!("seasons.{key}"),
                                        "month is outside 1..=12",
                                    )
                                })?,
                        )
                    } else {
                        1
                    };
                    let animal_count = u128::from(if good_id == "dairy" {
                        building.herds[0]
                    } else {
                        building.herds[2]
                    });
                    let numerator = animal_count
                        .checked_mul(u128::from(base))
                        .and_then(|value| value.checked_mul(season_share))
                        .and_then(|value| value.checked_mul(u128::from(coverage_efficiency)))
                        .and_then(|value| {
                            value.checked_add(u128::from(building.herd_product_carries[good_index]))
                        })
                        .ok_or_else(|| {
                            missing_data(
                                format!("herds.products.{good_id}"),
                                "product numerator overflow",
                            )
                        })?;
                    let quantity =
                        u64::try_from(numerator / u128::from(denominator)).map_err(|_| {
                            missing_data(format!("herds.products.{good_id}"), "product exceeds u64")
                        })?;
                    building.herd_product_carries[good_index] =
                        u64::try_from(numerator % u128::from(denominator)).map_err(|_| {
                            missing_data(
                                format!("herds.products.{good_id}"),
                                "product carry exceeds u64",
                            )
                        })?;
                    monthly_output[good_index] = monthly_output[good_index]
                        .checked_add(quantity)
                        .ok_or_else(|| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.stock.{good_id}",
                                place.id, building.kind
                            ),
                            "herd product stock overflow",
                        )
                    })?;
                }
            }
            for (good_index, output_units) in monthly_output.iter().copied().enumerate() {
                place_ledger.produced[good_index] = place_ledger.produced[good_index]
                    .checked_add(output_units)
                    .ok_or_else(|| {
                        missing_data("ledger.production".to_owned(), "production total overflow")
                    })?;
            }
            let wage_share_pm = u64::from(1000_u16.saturating_sub(definition.overhead_pm)) * 1000;
            let worker_weights = building
                .employment
                .iter()
                .map(|employment| employment.workers)
                .collect::<Vec<_>>();
            let mut wage_value = 0_u128;
            for (good_index, output_units) in monthly_output.iter().copied().enumerate() {
                let split_num = u128::from(output_units)
                    .checked_mul(u128::from(wage_share_pm))
                    .and_then(|value| {
                        value.checked_add(u128::from(building.wage_carries[good_index]))
                    })
                    .ok_or_else(|| {
                        missing_data("payday.split".to_owned(), "wage split overflow")
                    })?;
                let wage = u64::try_from(split_num / 1_000_000).map_err(|_| {
                    missing_data("payday.split".to_owned(), "wage units exceed u64")
                })?;
                let price = place.prices_milli.get(good_index).copied().ok_or_else(|| {
                    missing_data(
                        format!("places.{}.prices_milli", place.id),
                        "saved prices do not match goods",
                    )
                })?;
                wage_value = wage_value
                    .checked_add(value_at_price(u128::from(wage), price).map_err(|problem| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.wage_history",
                                place.id, building.kind
                            ),
                            problem,
                        )
                    })?)
                    .ok_or_else(|| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.wage_history",
                                place.id, building.kind
                            ),
                            "payday wage value overflow",
                        )
                    })?;
                building.wage_carries[good_index] =
                    u64::try_from(split_num % 1_000_000).map_err(|_| {
                        missing_data("payday.split".to_owned(), "wage carry exceeds u64")
                    })?;
                let overhead = output_units.checked_sub(wage).ok_or_else(|| {
                    missing_data("payday.split".to_owned(), "wage exceeds output")
                })?;
                let unpaid = building.pay_carries[good_index]
                    .checked_add(wage)
                    .ok_or_else(|| missing_data("payday.carry".to_owned(), "pay carry overflow"))?;
                let mut contributions = Vec::new();
                if worker_weights.iter().all(|count| *count == 0) {
                    building.pay_carries[good_index] = unpaid;
                } else {
                    building.pay_carries[good_index] = 0;
                    let shares = largest_remainder(unpaid, &worker_weights).map_err(|problem| {
                        missing_data("payday.distribution".to_owned(), problem)
                    })?;
                    for (pop_index, share) in shares.into_iter().enumerate() {
                        if share == 0 {
                            continue;
                        }
                        let pop = place.pops.get_mut(pop_index).ok_or_else(|| {
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
                    pending_overflow.push((building_index, good_index, contributions));
                }
            }
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
        for building_index in fee_buildings {
            pay_storehouse_fee(
                rules,
                &place.id,
                &place.prices_milli,
                usize::from(date.month - 1),
                &mut place.pops,
                &mut place.buildings,
                building_index,
                &mut place.counters,
                &mut place_ledger,
            )?;
        }
        place_ledger.stock_after = total_stored_goods(rules, place)?;
        place_ledger.herds_after = total_herds(place)?;
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
    let mut next = state.clone();
    update_daily_prices(rules, &mut next)?;
    let mut ledger = run_monthly_production_with_quality(rules, &mut next, date, quality_by_place)?;
    let (intake_pm, daily_consumption) = run_daily_consumption_with_ledger(rules, &mut next, date)?;
    ledger.intake_pm = intake_pm;
    ledger.daily_consumption = Some(daily_consumption);
    *state = next;
    Ok(ledger)
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
            place.coastal.encode(out)?;
            encode_length(place.pops.len(), out)?;
            for pop in &place.pops {
                pop.kind.encode(out)?;
                pop.culture.encode(out)?;
                pop.religion.encode(out)?;
                pop.workers.encode(out)?;
                pop.dependents.encode(out)?;
                encode_employment(&pop.employment, out)?;
                pop.stock.encode(out)?;
                encode_pop_consumption(&pop.consumption, out)?;
                encode_counters(&pop.counters, out)?;
            }
            encode_length(place.buildings.len(), out)?;
            for building in &place.buildings {
                building.kind.encode(out)?;
                building.levels.encode(out)?;
                building.method_levels.encode(out)?;
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
            }
            place.prices_milli.encode(out)?;
            place.prepared_m2.encode(out)?;
            place.development_pm.encode(out)?;
            place.price_drift_carries.encode(out)?;
            encode_counters(&place.counters, out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let place_count = input.length()?;
        let mut places = Vec::with_capacity(place_count);
        for _ in 0..place_count {
            let id = String::decode(input)?;
            let coastal = bool::decode(input)?;
            let count = input.length()?;
            let mut pops = Vec::with_capacity(count);
            for _ in 0..count {
                pops.push(PopState {
                    kind: String::decode(input)?,
                    culture: String::decode(input)?,
                    religion: String::decode(input)?,
                    workers: u64::decode(input)?,
                    dependents: u64::decode(input)?,
                    employment: decode_employment(input)?,
                    stock: Vec::<u64>::decode(input)?,
                    consumption: decode_pop_consumption(input)?,
                    counters: decode_counters(input)?,
                });
            }
            let count = input.length()?;
            let mut buildings = Vec::with_capacity(count);
            for _ in 0..count {
                buildings.push(BuildingState {
                    kind: String::decode(input)?,
                    levels: u32::decode(input)?,
                    method_levels: Vec::<u32>::decode(input)?,
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
                    production_carries: Vec::<Vec<u64>>::decode(input)?,
                    method_suitability: Vec::<Vec<u64>>::decode(input)?,
                    herd_birth_carries: Vec::<u64>::decode(input)?,
                    herd_death_carries: Vec::<u64>::decode(input)?,
                    herd_product_carries: Vec::<u64>::decode(input)?,
                    herd_grazing_sums: Vec::<u64>::decode(input)?,
                    wage_carries: Vec::<u64>::decode(input)?,
                    pay_carries: Vec::<u64>::decode(input)?,
                    fee_carries: Vec::<Vec<u64>>::decode(input)?,
                    wage_history: Vec::<u64>::decode(input)?,
                });
            }
            places.push(PlaceState {
                id,
                coastal,
                pops,
                buildings,
                prices_milli: Vec::<u64>::decode(input)?,
                prepared_m2: u64::decode(input)?,
                development_pm: u32::decode(input)?,
                price_drift_carries: Vec::<i64>::decode(input)?,
                counters: decode_counters(input)?,
            });
        }
        Ok(Self { places })
    }
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
mod tests {
    use super::*;

    #[test]
    fn buildable_area_uses_all_land_and_scales_with_checked_development() {
        let rules = tiny_rules();
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 1)]),
            fertility_pm: BTreeMap::new(),
            fertile_cells: 0,
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

    fn tiny_rules() -> Rules {
        Rules {
            goods: vec![Good {
                id: "food".to_owned(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
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
            }],
            starting_needs_pm: BTreeMap::from([("peasant".to_owned(), [1000, 0, 0])]),
            cultures: vec!["connachta".to_owned()],
            religions: vec!["christianity".to_owned()],
            terrain_order: vec!["pasture".to_owned(), "bog".to_owned()],
            footprint_m2: BTreeMap::from([
                ("farm".to_owned(), 2000),
                ("storehouse".to_owned(), 200),
            ]),
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
                    workers_per_level: 10,
                    land_per_level: 2,
                    owner_workers: BTreeMap::from([("peasant".to_owned(), 10)]),
                    penalty_pm: 500,
                    overhead_pm: 100,
                    methods: vec![Method {
                        id: "mixed".to_owned(),
                        resource: "usable_land".to_owned(),
                        annual_output: BTreeMap::from([("food".to_owned(), 1)]),
                        suitability: "crops".to_owned(),
                        score_share_pm: BTreeMap::from([("food".to_owned(), 1000)]),
                    }],
                },
                BuildingType {
                    id: "storehouse".to_owned(),
                    workers_per_level: 2,
                    land_per_level: 0,
                    owner_workers: BTreeMap::new(),
                    penalty_pm: 0,
                    overhead_pm: 0,
                    methods: Vec::new(),
                },
            ],
            seasons: BTreeMap::from([(
                "grain".to_owned(),
                vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0],
            )]),
            curves: BTreeMap::from([(
                "intake_food_days".to_owned(),
                vec![[30, 500], [120, 800], [240, 1000]],
            )]),
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
            grazing: BTreeMap::from([(
                "pasture".to_owned(),
                BTreeMap::from([
                    ("cattle".to_owned(), 1000),
                    ("pigs".to_owned(), 1000),
                    ("sheep".to_owned(), 1000),
                ]),
            )]),
            price_k_pm: 500,
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
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
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
        let goods = rules.goods.len();
        let make_pop = |necessities_pm| PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
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
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
            herd_product_carries: vec![0; goods],
            herd_grazing_sums: vec![1000; 3],
            wage_carries: vec![0; 2],
            pay_carries: vec![0; 2],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
        };
        drawer.employment = vec![Employment::default(); 2];
        starver.employment = vec![Employment::default(); 2];
        let state = EconomyState {
            places: vec![PlaceState {
                id: "daily_fixture".to_owned(),
                coastal: false,
                pops: vec![drawer, starver],
                buildings: vec![storehouse, farm],
                prices_milli: vec![1000; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; goods],
                counters: CounterBuckets::default(),
            }],
        };
        (rules, state)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "The shipped production fixture keeps all rules-indexed state dimensions visible to its literal season test."
    )]
    fn shipped_production_fixture(rules: &Rules) -> EconomyState {
        let goods = rules.goods.len();
        let empty_stock = vec![0; goods];
        let pop = |kind: &str, workers, employment| PopState {
            kind: kind.to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            workers,
            dependents: 0,
            employment,
            stock: empty_stock.clone(),
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        EconomyState {
            places: vec![PlaceState {
                id: "fixture".to_owned(),
                coastal: false,
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
                        method_levels: vec![1, 0],
                        herds: vec![9, 6, 5],
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
                        production_carries: vec![vec![0; goods]; 2],
                        method_suitability: vec![vec![1000; goods]; 2],
                        herd_birth_carries: vec![0; 3],
                        herd_death_carries: vec![0; 3],
                        herd_product_carries: vec![0; goods],
                        herd_grazing_sums: vec![1000, 400, 1000],
                        wage_carries: vec![0; goods],
                        pay_carries: vec![0; goods],
                        fee_carries: Vec::new(),
                        wage_history: vec![0; 12],
                    },
                    BuildingState {
                        kind: "holdings".to_owned(),
                        levels: 1,
                        method_levels: vec![0, 1],
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
                    },
                    BuildingState {
                        kind: "storehouse".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 1, 1],
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
                    },
                ],
                prices_milli: vec![0; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                counters: CounterBuckets::default(),
            }],
        }
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
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        assert_eq!(receive_pop(&rules, &mut pop, 0, 4).unwrap(), 3);
        assert_eq!(pop.stock, [1]);

        rules.goods[0].pop_limit_milli = 1_000_000;
        let mut farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
    fn overflow_moves_from_building_through_alms_then_source_accounts() {
        let mut rules = tiny_rules();
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].volume_ml = 100_000;
        rules.goods[0].pop_limit_milli = 1_000_000;
        let pop = |kind: &str| PopState {
            kind: kind.to_owned(),
            culture: "ulaid".to_owned(),
            religion: "christianity".to_owned(),
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![7],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        let mut pops = vec![pop("peasant"), pop("labourer")];
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
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
        };
        let mut buildings = vec![farm, storehouse];
        let mut ledger = PlaceProductionLedger::new("fort", 1);
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
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![food],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
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
            workers: 10,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0; rules.goods.len()],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        let place = PlaceState {
            id: "luxury_market".to_owned(),
            coastal: true,
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
        assert_eq!(
            food_target_units(&rules, "monk", 10, ale, true, 240),
            Ok(50)
        );
        assert_eq!(
            tier_target_units(&rules, "monk", 10, ale, 1)
                .unwrap()
                .max(tier_target_units(&rules, "monk", 10, ale, 2).unwrap()),
            213
        );
        let oats_target = food_target_units(&rules, "monk", 10, oats, true, 240).unwrap();
        let place_with = |ale_holding| {
            let mut stock = vec![0; rules.goods.len()];
            stock[ale] = ale_holding;
            stock[oats] = u64::try_from(oats_target + 7).unwrap();
            PlaceState {
                id: "comfort_order".to_owned(),
                coastal: true,
                pops: vec![PopState {
                    kind: "monk".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    workers: 10,
                    dependents: 0,
                    employment: Vec::new(),
                    stock,
                    consumption: PopConsumption::default(),
                    counters: CounterBuckets::default(),
                }],
                buildings: Vec::new(),
                prices_milli: vec![0; rules.goods.len()],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; rules.goods.len()],
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
            workers: people,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        }
    }

    fn reserve_test_building(owners: u64, food_stock: u64) -> BuildingState {
        BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![0],
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
        }
    }

    fn reserve_test_place(pop: PopState, buildings: Vec<BuildingState>) -> PlaceState {
        PlaceState {
            id: "owner_reserve".to_owned(),
            coastal: false,
            pops: vec![pop],
            buildings,
            prices_milli: vec![0],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0],
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
            workers: 10,
            dependents: 0,
            employment: vec![Employment::default(); 2],
            stock: vec![0; rules.goods.len()],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
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
        };
        let mut farm_stock = vec![0; rules.goods.len()];
        farm_stock[index("apples")] = 20;
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
        };
        let place = PlaceState {
            id: "market_fixture".to_owned(),
            coastal: true,
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
            counters: CounterBuckets::default(),
        };
        assert_account_order_targets(&rules, &place);
    }

    #[test]
    fn a_balanced_market_holds_base_price_and_drift_keeps_its_fractional_carry() {
        let rules = tiny_rules();
        let target = food_target_units(&rules, "peasant", 1, 0, false, 240).unwrap();
        assert_eq!(target, 240);
        let place = PlaceState {
            id: "balanced".to_owned(),
            coastal: false,
            pops: vec![PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
                workers: 1,
                dependents: 0,
                employment: Vec::new(),
                stock: vec![u64::try_from(target).unwrap()],
                consumption: PopConsumption::default(),
                counters: CounterBuckets::default(),
            }],
            buildings: Vec::new(),
            prices_milli: vec![100_000],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0],
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

        state.places[0].pops[0].stock[0] = 0;
        state.places[0].prices_milli[0] = 100_001;
        update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli, [105_000]);
        assert_eq!(state.places[0].price_drift_carries, [900]);
        update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli, [109_500]);
        assert_eq!(state.places[0].price_drift_carries, [900]);
    }

    #[test]
    fn shipped_economy_and_ireland_start_data_are_strictly_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(root.join("game/economy.toml")).unwrap();
        let starts =
            ScenarioEconomy::load(root.join("scenarios/ireland-768/economy.toml")).unwrap();
        assert_eq!(starts.places.len(), 94);
        assert!(starts.places.values().all(|place| {
            place.culture.is_some()
                && place.religion.as_deref() == Some("christianity")
                && place.monk_religion.as_deref() == Some("christianity")
        }));
        assert_eq!(rules.goods.len(), 22);
        assert_eq!(rules.pop_types.len(), 3);
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
            Some("english")
        );
        assert_eq!(starts.places["skellig_michael"].levels, Some([0, 2, 1]));
        assert_eq!(starts.places["skellig_michael"].peasants, Some(0));
        assert_eq!(starts.places["inisfallen"].levels, Some([0, 2, 1]));
        assert_eq!(starts.places["inisfallen"].peasants, Some(0));
        assert_eq!(starts.places["inishmurray"].peasants, Some(60));
        for (id, share) in [
            ("cahercommaun", 200),
            ("killala", 150),
            ("cong", 150),
            ("tory", 100),
        ] {
            assert_eq!(starts.places[id].pagan_share_pm, Some(share));
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
                ("peasant".to_owned(), 1_000),
            ])
        );
        assert_eq!(
            rules.ticking_rates_million["beeswax"],
            BTreeMap::from([
                ("monk".to_owned(), 4_000),
                ("labourer".to_owned(), 500),
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
            87
        );
        assert_eq!(state.places[0].counters.current.losses, 87);
        assert_eq!(state.places[0].pops[1].counters.current.losses, 87);
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
        }
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
                counters: CounterBuckets::default(),
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
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
                }],
                buildings: vec![
                    BuildingState {
                        kind: "farm".to_owned(),
                        levels: 1,
                        method_levels: vec![1],
                        herds: vec![9, 6, 5],
                        employment: vec![Employment {
                            owners: 1,
                            workers: 10
                        }],
                        stock: vec![2],
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
                        wage_history: vec![1818; 12],
                    },
                    BuildingState {
                        kind: "storehouse".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 0, 0],
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
        assert_eq!(farm.posted_wage(), (11 * 9000 + 1234) / 12);

        let farm = state.places[0]
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        farm.wage_history[1] = 7777;
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
        let storehouse = &place.buildings[storehouse_index];
        assert_eq!(storehouse.wage_history[0], 1500);
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
        storehouse.employment[0].workers = 0;
        place.pops[0].employment[storehouse_index].workers = 0;

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
        let storehouse = &state.places[0].buildings[storehouse_index];
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
        place.prices_milli[0] = u64::MAX;
        let storehouse = &mut place.buildings[storehouse_index];
        storehouse.wage_history[0] = 1234;
        storehouse.employment[0].workers = 1;
        place.pops[0].employment[storehouse_index].workers = 1;
        storehouse.stock[0] = 200;
        storehouse.storehouse_accounts.as_mut().unwrap().pops[0][0] = 200;
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
    fn monthly_production_keeps_fractional_seasonal_output_and_skips_other_days() {
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
        assert_eq!(state.places[0].buildings[storehouse].stock[0], 1_980);
        assert_eq!(
            state.places[0].buildings[storehouse]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0][0],
            990
        );
        assert_eq!(
            state.places[0].buildings[storehouse]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal[0],
            990
        );
        assert_eq!(
            state.places[0].buildings[farm].production_carries[0][0],
            5_000_000_000
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
            let mut final_carry = 0_u64;
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
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let mut reference = shipped_production_fixture(&rules);
        reference.places[0].buildings[1].method_levels = vec![1, 0];
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
        assert_eq!(state.places[0].counters.current, MonthlyCounters::default());
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
    fn mixed_peasant_cultures_are_split_by_authored_per_mille_shares() {
        let mut rules = tiny_rules();
        rules.cultures.push("mide_brega".to_owned());
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
                workers_per_level: 10,
                land_per_level: 2,
                owner_workers: BTreeMap::from([
                    ("monk".to_owned(), 5),
                    ("labourer".to_owned(), 10),
                ]),
                penalty_pm: 500,
                overhead_pm: 300,
                methods: vec![rules.buildings[0].methods[0].clone()],
            },
        );
        rules.footprint_m2.insert("holdings".to_owned(), 4000);
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
        let rules = tiny_rules();
        let method = &rules.buildings[0].methods[0];
        assert_eq!(
            starting_method_output(&rules, method, "beef", 1100, Some("pasture"), 550),
            Ok(825)
        );
        assert_eq!(
            starting_method_output(&rules, method, "food", 300, Some("pasture"), 550),
            Ok(300)
        );
        assert_eq!(
            starting_method_output(&rules, method, "food", 300, Some("pasture"), 275),
            Ok(150)
        );
        assert!(
            starting_method_output(&rules, method, "food", 300, Some("pasture"), 275).unwrap()
                < starting_method_output(&rules, method, "food", 300, Some("pasture"), 550)
                    .unwrap()
        );
    }

    #[test]
    fn inland_diet_profiles_renormalize_in_good_order_after_removing_fish() {
        let mut rules = tiny_rules();
        rules.goods.extend([
            Good {
                id: "grain_2".to_owned(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                hunger_centi: 100,
                necessities_centi: 100,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 100,
                pop_limit_milli: 1000,
            },
            Good {
                id: "fish".to_owned(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
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
            normalized_diet_shares(&rules, rules.diet_profiles.get("peasant").unwrap(), false)
                .unwrap();
        assert_eq!(profile, [500, 500, 0]);
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
            workers: 12,
            dependents: 8,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        assert_eq!(
            projected_buy_units(&rules, std::slice::from_ref(&pop), "food", true),
            Ok(7300)
        );
        assert_eq!(
            projected_buy_units(&rules, std::slice::from_ref(&pop), "food", false),
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
            sea: 0,
        };
        let refused = assign_building_levels(
            &rules,
            building,
            1,
            &mut just_short,
            &mut remaining_sea,
            &BTreeMap::new(),
            &[],
            false,
            550,
        )
        .unwrap();
        assert_eq!(refused.method_levels, [0]);
        assert_eq!(just_short.fertility_pm["pasture"], 1999);

        let mut exact_room = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
            fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
            fertile_cells: 2,
            sea: 0,
        };
        let accepted = assign_building_levels(
            &rules,
            building,
            1,
            &mut exact_room,
            &mut remaining_sea,
            &BTreeMap::new(),
            &[],
            false,
            550,
        )
        .unwrap();
        assert_eq!(accepted.method_levels, [1]);
        assert_eq!(exact_room.fertility_pm["pasture"], 0);
    }

    #[test]
    fn coastal_methods_need_nearby_sea_but_one_sea_cell_allows_many_levels() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "fish".to_owned(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
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
            annual_output: BTreeMap::from([("fish".to_owned(), 1000)]),
            suitability: "sea".to_owned(),
            score_share_pm: BTreeMap::from([("fish".to_owned(), 1000)]),
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
        assert_eq!(
            farm.method_levels[1], 3,
            "fishing levels are not capped by sea/4"
        );
        assert_eq!(farm.levels, 3);
    }

    #[test]
    fn holdings_take_shared_farming_land_before_the_farm() {
        let mut rules = tiny_rules();
        let farm_method = rules.buildings[0].methods[0].clone();
        rules.buildings.insert(
            1,
            BuildingType {
                id: "holdings".to_owned(),
                workers_per_level: 10,
                land_per_level: 2,
                owner_workers: BTreeMap::from([
                    ("monk".to_owned(), 5),
                    ("labourer".to_owned(), 10),
                ]),
                penalty_pm: 500,
                overhead_pm: 300,
                methods: vec![farm_method],
            },
        );
        rules.footprint_m2.insert("holdings".to_owned(), 4000);
        rules.pop_types.push(PopType {
            id: "monk".to_owned(),
            dependent_pm: 100,
            births_pm_year: 0,
            worker_deaths_pm_year: 20,
            dependent_deaths_pm_year: 60,
            necessities_expectation_pm: 1000,
            niceties_expectation_pm: 0,
            luxuries_expectation_pm: 0,
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
        });
        add_empty_comfort_profile(&mut rules, "labourer");
        rules
            .starting_needs_pm
            .insert("labourer".to_owned(), [1000, 0, 0]);
        rules.diet_profiles.insert(
            "labourer".to_owned(),
            BTreeMap::from([("food".to_owned(), 1000)]),
        );
        rules.cultures.push("english".to_owned());
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
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml,
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
                counters: CounterBuckets::default(),
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "irish".to_owned(),
                    religion: "christianity".to_owned(),
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
                    },
                    counters: CounterBuckets::default(),
                }],
                buildings: vec![
                    BuildingState {
                        kind: "farm".to_owned(),
                        levels: 1,
                        method_levels: vec![1],
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
                    },
                    BuildingState {
                        kind: "storehouse".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 0, 0],
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
                    },
                ],
                prices_milli: vec![120_000],
                prepared_m2: 23_456,
                development_pm: 875,
            }],
        };
        let bytes = state.to_bytes().unwrap();
        let decoded = EconomyState::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, state);
        assert_eq!(
            decoded.places[0].buildings[0].wage_history,
            (1..=12).collect::<Vec<_>>()
        );
        assert_eq!(decoded.places[0].buildings[0].posted_wage(), 6);
        assert_eq!(decoded.to_bytes().unwrap(), bytes);

        let mut rules = tiny_rules();
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
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
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
        farm.herd_product_carries[rules
            .goods
            .iter()
            .position(|good| good.id == "wool")
            .unwrap()] = 4_999;

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

        let pop = |workers, employment| PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            workers,
            dependents: 0,
            employment,
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        let building = |kind: &str, wage_carry| BuildingState {
            kind: kind.to_owned(),
            levels: 1,
            method_levels: vec![1],
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
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "fort".to_owned(),
                coastal: false,
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
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1_000_000,
        });
        rules.goods.push(Good {
            id: "hides".to_owned(),
            unit: "hide".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1_000_000,
        });
        let no_workers = EconomyState {
            places: vec![PlaceState {
                id: "ownerless".to_owned(),
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    workers: 0,
                    dependents: 0,
                    employment: vec![Employment::default()],
                    stock: vec![0; rules.goods.len()],
                    consumption: PopConsumption::default(),
                    counters: CounterBuckets::default(),
                }],
                buildings: vec![BuildingState {
                    kind: "farm".to_owned(),
                    levels: 1,
                    method_levels: vec![1],
                    herds: vec![20, 0, 0],
                    employment: vec![Employment::default()],
                    stock: vec![0; rules.goods.len()],
                    storehouse_accounts: None,
                    production_carries: vec![vec![0; rules.goods.len()]],
                    method_suitability: vec![vec![1000; rules.goods.len()]],
                    herd_birth_carries: vec![0; 3],
                    herd_death_carries: vec![0; 3],
                    herd_product_carries: vec![0; rules.goods.len()],
                    herd_grazing_sums: vec![0; 3],
                    wage_carries: vec![0; rules.goods.len()],
                    pay_carries: vec![0; rules.goods.len()],
                    fee_carries: Vec::new(),
                    wage_history: vec![0; 12],
                }],
                prices_milli: vec![0; rules.goods.len()],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
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
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
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
        assert_eq!(ledger.herd_births, [1, 0, 0]);
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        assert_eq!(ledger.produced[fish], 232);

        let mut harvest = shipped_production_fixture(&rules);
        harvest.places[0].buildings[1].method_levels = vec![1, 0];
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
    fn overflow_pays_alms_then_records_source_and_place_loss() {
        let mut rules = tiny_rules();
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].volume_ml = 1;
        rules.goods[0].pop_limit_milli = 1000;
        let pop = |kind: &str, food| PopState {
            kind: kind.to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![food],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
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
        };
        let mut pops = vec![pop("labourer", 1), pop("peasant", 0)];
        let mut buildings = vec![farm, storehouse];
        let mut counters = CounterBuckets::default();
        let mut ledger = PlaceProductionLedger::new("fort", 1);
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
    fn monthly_satisfaction_drift_runs_on_day_one_only() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
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
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "empty".to_owned(),
                coastal: false,
                pops: vec![pop],
                buildings: Vec::new(),
                prices_milli: vec![1000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                counters: CounterBuckets::default(),
            }],
        };
        run_daily_consumption(
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
        assert_eq!(state.places[0].pops[0].consumption.satisfaction, 6);
    }

    #[test]
    fn daily_eating_opens_only_whole_goods_and_drifts_needs() {
        let rules = tiny_rules();
        let goods = rules.goods.len();
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
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
                }],
                buildings: Vec::new(),
                prices_milli: vec![100_000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
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
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
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
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000; 2],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
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
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
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
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
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
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; goods],
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
        assert_eq!(pop.stock[1], 1);
        assert_eq!(pop.consumption.niceties_pm, 1000);
    }

    #[test]
    fn missing_ticking_rate_is_refused_by_good_name() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "herbs".to_owned(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
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
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                coastal: false,
                pops: vec![pop.clone()],
                buildings: Vec::new(),
                prices_milli: vec![1000, 1000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
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
        };
        pop.employment.push(Employment {
            owners: 1,
            workers: 0,
        });
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
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
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
        };
        let mut place = PlaceState {
            id: "test".to_owned(),
            coastal: false,
            pops: vec![pop],
            buildings: vec![storehouse, farm],
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: Vec::new(),
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
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
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
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
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
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000, 1000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
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
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
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
                coastal: false,
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
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
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000; 3],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
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
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                hunger_centi: hunger,
                necessities_centi: hunger,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 1,
                pop_limit_milli: 100_000,
            });
        }
        let make = |necessities_pm| {
            let goods = rules.goods.len();
            let pop = PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
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
            };
            let building = BuildingState {
                kind: "farm".to_owned(),
                levels: 1,
                method_levels: vec![1],
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
            };
            PlaceState {
                id: "test".to_owned(),
                coastal: false,
                pops: vec![pop],
                buildings: vec![building],
                prices_milli: vec![1000; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; goods],
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
        assert_eq!(ledger.lost[pork], 76);
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
        assert_eq!(ledger.lost[beef], 120);
    }

    #[test]
    fn emergency_butchering_skips_owned_buildings_without_herds() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
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
        };
        let building = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
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
        };
        let mut place = PlaceState {
            id: "test".to_owned(),
            coastal: false,
            pops: vec![pop],
            buildings: vec![building],
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: Vec::new(),
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
}
