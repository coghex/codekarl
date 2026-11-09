//! Validated economy data and deterministic M9 starting-state primitives.
//!
//! This crate is independent of maps, windows, Lua and renderers. The application
//! supplies authored scenario data and derived land counts through plain integers.

#![deny(unsafe_code)]

use std::{collections::BTreeMap, error::Error, fmt, fs, path::Path};

use ck_sim::{Canonical, CanonicalError, Reader, encode_length};
use serde::Deserialize;

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
    /// Food diet shares in table order; validation requires 1000 after normalization.
    pub diet: BTreeMap<String, u16>,
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
    /// Cultures present in the simulation.
    pub cultures: Vec<String>,
    /// Religions present in the simulation.
    pub religions: Vec<String>,
    /// Land terrain names in the same order as the map terrain palette.
    pub terrain_order: Vec<String>,
    /// Building definitions.
    pub buildings: Vec<BuildingType>,
    /// Seasonal shares by product, each summing to 1000.
    pub seasons: BTreeMap<String, Vec<u16>>,
    /// Response curves by name, with strictly increasing x coordinates.
    pub curves: BTreeMap<String, Vec<[i32; 2]>>,
    /// Diet shares by profile and good, in ‰.
    pub diet_profiles: BTreeMap<String, BTreeMap<String, u16>>,
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
            if pop.id.is_empty()
                || pop.dependent_pm > 1000
                || pop.diet.values().copied().map(u32::from).sum::<u32>() != 1000
            {
                return Err(refuse(
                    format!("pop_types[{index}]"),
                    "invalid id, dependent share or diet total (must sum to 1000)".to_owned(),
                ));
            }
            for good in pop.diet.keys() {
                if !goods.contains(good.as_str()) {
                    return Err(refuse(
                        format!("pop_types[{index}].diet.{good}"),
                        "unknown good".to_owned(),
                    ));
                }
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
    /// Usable cells by terrain key.
    pub usable: BTreeMap<String, u32>,
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
) -> Result<(EconomyState, Vec<String>), DataError> {
    rules.validate("economy rules")?;
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
        let mut farm_land_total = land
            .usable
            .values()
            .try_fold(0_u32, |total, cells| total.checked_add(*cells))
            .ok_or_else(|| DataError {
                file: "map land counts".to_owned(),
                entry: location.id.clone(),
                problem: "usable cell count overflow".to_owned(),
            })?;
        let mut production = BTreeMap::<String, u64>::new();
        // Holdings take farming land before the farm, while coastal levels share
        // the same finite sea-cell budget across both buildings.
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
                &mut farm_land_total,
                &production,
                &pops,
                location.land.sea > 0,
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
            buildings.push(BuildingState {
                kind: kind.to_owned(),
                levels: actual,
                method_levels: assignment.method_levels,
                herds,
                employment: vec![Employment::default(); pops.len()],
                stock: stock_vector(rules, &defaults.building_stock),
                storehouse_accounts: None,
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
        state.places.push(PlaceState {
            id: location.id.clone(),
            pops,
            buildings,
            prices_milli,
        });
    }
    Ok((state, report))
}

fn missing_data(entry: String, problem: &str) -> DataError {
    DataError {
        file: "economy rules".to_owned(),
        entry,
        problem: problem.to_owned(),
    }
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
    Ok(PopState {
        kind: kind.to_owned(),
        culture: culture.to_owned(),
        religion: religion.to_owned(),
        workers,
        dependents,
        employment: Vec::new(),
        stock,
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
    farming_cells: &mut u32,
    prior_sell: &BTreeMap<String, u64>,
    pops: &[PopState],
    coastal: bool,
) -> Result<BuildingAssignment, DataError> {
    let mut assigned = vec![0_u32; building.methods.len()];
    let mut used_land = 0_u32;
    let mut used_sea = 0_u32;
    let mut output = BTreeMap::<String, u64>::new();
    for _ in 0..wanted {
        let mut best: Option<(usize, u128, Option<String>)> = None;
        for (index, method) in building.methods.iter().enumerate() {
            let terrain = if method.resource == "sea" {
                if *remaining_sea < 4 {
                    continue;
                }
                None
            } else {
                if *farming_cells < u32::from(building.land_per_level) {
                    continue;
                }
                choose_terrain(rules, method, land)
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
                let suitability = if method.resource == "sea" {
                    1000_u128
                } else {
                    terrain
                        .as_ref()
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
                let output_units = u128::from(*base_output)
                    .checked_mul(suitability)
                    .ok_or_else(|| {
                        missing_data(
                            format!("buildings.{}.methods.{}", building.id, method.id),
                            "annual output estimate overflow",
                        )
                    })?
                    / 1000;
                let sell_before = u128::from(prior_sell.get(good_id).copied().unwrap_or(0))
                    .checked_add(u128::from(output.get(good_id).copied().unwrap_or(0)))
                    .ok_or_else(|| missing_data("projection".to_owned(), "supply overflow"))?;
                let buy_units = projected_buy_units(rules, pops, good_id, coastal)?;
                let buy_value = buy_units
                    .checked_mul(u128::from(good.base_price))
                    .ok_or_else(|| {
                        missing_data("projection".to_owned(), "demand value overflow")
                    })?;
                let sell_value = sell_before
                    .checked_mul(u128::from(good.base_price))
                    .ok_or_else(|| {
                        missing_data("projection".to_owned(), "supply value overflow")
                    })?;
                let price = target_price(good.base_price, buy_value, sell_value, rules.price_k_pm)
                    .map_err(|problem| missing_data(format!("target_price.{good_id}"), problem))?;
                estimate = estimate
                    .checked_add(output_units.checked_mul(u128::from(price)).ok_or_else(|| {
                        missing_data("projection".to_owned(), "method value overflow")
                    })?)
                    .ok_or_else(|| {
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
        if method.resource == "sea" {
            *remaining_sea -= 4;
            used_sea += 4;
        } else if let Some(terrain) = terrain.as_ref() {
            let cells = land
                .usable
                .get_mut(terrain)
                .expect("selected terrain exists");
            *cells -= u32::from(building.land_per_level);
            *farming_cells -= u32::from(building.land_per_level);
            used_land += u32::from(building.land_per_level);
        }
        for (good_id, quantity) in &method.annual_output {
            let suitability = if method.resource == "sea" {
                1000_u128
            } else {
                terrain
                    .as_ref()
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
            let produced = u128::from(*quantity)
                .checked_mul(suitability)
                .ok_or_else(|| missing_data("projection".to_owned(), "annual output overflow"))?
                / 1000;
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
        used_land,
        used_sea,
        output,
    })
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
        let share = if !coastal && good_id == "fish" {
            0
        } else {
            u128::from(shares.get(good_id).copied().unwrap_or(0))
        };
        let normalizer: u128 = if coastal {
            shares.values().map(|value| u128::from(*value)).sum()
        } else {
            shares
                .iter()
                .filter(|(id, _)| id.as_str() != "fish")
                .map(|(_, value)| u128::from(*value))
                .sum()
        };
        if normalizer == 0 {
            continue;
        }
        let people = pop
            .workers
            .checked_add(pop.dependents)
            .ok_or_else(|| missing_data("projection".to_owned(), "population sum overflow"))?;
        let numerator = u128::from(people)
            .checked_mul(365)
            .and_then(|value| value.checked_mul(100))
            .and_then(|value| value.checked_mul(share))
            .ok_or_else(|| missing_data("projection".to_owned(), "diet demand overflow"))?;
        let denominator = normalizer
            .checked_mul(u128::from(good.hunger_centi))
            .ok_or_else(|| missing_data("projection".to_owned(), "diet denominator overflow"))?;
        total = total
            .checked_add(numerator / denominator)
            .ok_or_else(|| missing_data("projection".to_owned(), "diet demand sum overflow"))?;
    }
    Ok(total)
}

fn choose_terrain(rules: &Rules, method: &Method, land: &LandCounts) -> Option<String> {
    let mut best: Option<(u128, usize, String)> = None;
    for (index, terrain) in rules.terrain_order.iter().enumerate() {
        if land.usable.get(terrain).copied().unwrap_or(0) < 2 {
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

/// A location's economy state and saved local base prices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceState {
    /// Scenario-local location key.
    pub id: String,
    /// Pops in authored order.
    pub pops: Vec<PopState>,
    /// Buildings in authored order.
    pub buildings: Vec<BuildingState>,
    /// Saved prices in good order, represented in thousandths.
    pub prices_milli: Vec<u64>,
}

/// Canonical saved economy state; all lookup maps are represented in stable vectors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EconomyState {
    /// Places in scenario-file order.
    pub places: Vec<PlaceState>,
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
                pop.workers.encode(out)?;
                pop.dependents.encode(out)?;
                encode_employment(&pop.employment, out)?;
                pop.stock.encode(out)?;
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
            }
            place.prices_milli.encode(out)?;
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
                    workers: u64::decode(input)?,
                    dependents: u64::decode(input)?,
                    employment: decode_employment(input)?,
                    stock: Vec::<u64>::decode(input)?,
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
                });
            }
            places.push(PlaceState {
                id,
                pops,
                buildings,
                prices_milli: Vec::<u64>::decode(input)?,
            });
        }
        Ok(Self { places })
    }
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
                diet: BTreeMap::from([("food".to_owned(), 1000)]),
            }],
            cultures: vec!["connachta".to_owned()],
            religions: vec!["christianity".to_owned()],
            terrain_order: vec!["pasture".to_owned()],
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
            seasons: BTreeMap::new(),
            curves: BTreeMap::new(),
            diet_profiles: BTreeMap::from([(
                "peasant".to_owned(),
                BTreeMap::from([("food".to_owned(), 1000)]),
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
            places: ["fort", "island", "coast"]
                .into_iter()
                .map(|id| (id.to_owned(), ordinary_place()))
                .collect(),
        }
    }

    #[test]
    fn target_price_uses_the_named_k_and_floors_negative_ratios() {
        assert_eq!(target_price(120, 8, 7, 1000), Ok(127_999));
        assert_eq!(target_price(120, 8, 7, 500), Ok(123_999));
        assert_eq!(target_price(120, 7, 8, 1000), Ok(111_999));
        assert_eq!(target_price(120, 0, 0, 500), Ok(120_000));
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
    fn validation_names_the_file_entry_and_each_invalid_value() {
        let mut rules = tiny_rules();
        rules.pop_types[0].diet.insert("food".to_owned(), 999);
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(error.file, "fixture/economy.toml");
        assert_eq!(error.entry, "pop_types[0]");
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
    #[expect(
        clippy::too_many_lines,
        reason = "Keeping the refusal matrix together makes every named rule-validation branch auditable."
    )]
    fn rules_validation_refusals_are_table_driven_and_name_the_entry() {
        let cases: Vec<(&str, RuleMutation)> = vec![
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
                "pop_types[0].diet.missing",
                Box::new(|rules| {
                    rules.pop_types[0].diet.clear();
                    rules.pop_types[0].diet.insert("missing".to_owned(), 1000);
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
                starting_state(&rules, &data, &[location]).unwrap_err()
            };
            assert_eq!(error.entry, expected_entry, "case {expected_entry}");
        }
    }

    #[test]
    fn starting_state_is_literal_and_fills_owners_before_unlimited_starting_hires() {
        let rules = tiny_rules();
        let data = tiny_start_data();
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                usable: BTreeMap::from([("pasture".to_owned(), 2)]),
                sea: 0,
            },
        };
        let (state, report) = starting_state(&rules, &data, &[location]).unwrap();
        assert!(report.is_empty());
        assert_eq!(
            state.places,
            vec![PlaceState {
                id: "fort".to_owned(),
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
                    },
                ],
                prices_milli: vec![100_000],
            }]
        );
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
                diet: BTreeMap::from([("food".to_owned(), 1000)]),
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
                diet: BTreeMap::from([("food".to_owned(), 1000)]),
            },
        ]);
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
        defaults.storehouse_levels = 0;
        let (state, _) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    usable: BTreeMap::from([("pasture".to_owned(), 6)]),
                    sea: 0,
                },
            }],
        )
        .unwrap();
        let holdings = &state.places[0].buildings[0];
        assert_eq!(holdings.levels, 3);
        assert_eq!(holdings.employment[1].owners, 5);
        assert_eq!(holdings.employment[1].workers, 15);
        assert_eq!(holdings.employment[2].workers, 15);
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
            usable: BTreeMap::from([("pasture".to_owned(), 2), ("forest".to_owned(), 2)]),
            sea: 0,
        };
        assert_eq!(
            choose_terrain(&rules, method, &land).as_deref(),
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
            choose_terrain(&rules, method, &land).as_deref(),
            Some("pasture")
        );
    }

    #[test]
    fn coastal_method_uses_the_shared_sea_cap_and_fishing_has_no_herds() {
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
        rules.pop_types[0].diet.insert("food".to_owned(), 1000);
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
        let (state, report) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "coast".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    usable: BTreeMap::from([("pasture".to_owned(), 2)]),
                    sea: 4,
                },
            }],
        )
        .unwrap();
        assert_eq!(
            report,
            ["coast farm: cut levels 3 -> 2 (no method has enough land or sea)"]
        );
        let farm = &state.places[0].buildings[0];
        assert_eq!(
            farm.method_levels,
            [1, 1],
            "one level chooses fish, then the shared sea limit leaves the land method"
        );
        assert_eq!(farm.levels, 2);
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
        rules.pop_types.push(PopType {
            id: "monk".to_owned(),
            dependent_pm: 100,
            births_pm_year: 0,
            worker_deaths_pm_year: 20,
            dependent_deaths_pm_year: 60,
            necessities_expectation_pm: 1000,
            niceties_expectation_pm: 0,
            luxuries_expectation_pm: 0,
            diet: BTreeMap::from([("food".to_owned(), 1000)]),
        });
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
            diet: BTreeMap::from([("food".to_owned(), 1000)]),
        });
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
                    usable: BTreeMap::from([("pasture".to_owned(), 2)]),
                    sea: 0,
                },
            }],
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
    fn canonical_economy_round_trip_is_byte_stable() {
        let state = EconomyState {
            places: vec![PlaceState {
                id: "a".to_owned(),
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
                    },
                ],
                prices_milli: vec![120_000],
            }],
        };
        let bytes = state.to_bytes().unwrap();
        let decoded = EconomyState::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, state);
        assert_eq!(decoded.to_bytes().unwrap(), bytes);
    }
}
