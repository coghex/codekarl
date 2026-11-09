//! Validated economy data and deterministic M9 starting-state primitives.
//!
//! This crate is independent of maps, windows, Lua and renderers. The application
//! supplies authored scenario data and derived land counts through plain integers.

#![deny(unsafe_code)]

use std::{collections::BTreeMap, error::Error, fmt, fs, path::Path};

use ck_sim::{Canonical, CanonicalError, Date, Reader, encode_length};
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
        state.places.push(PlaceState {
            id: location.id.clone(),
            pops,
            buildings,
            prices_milli,
            counters: CounterBuckets::default(),
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
    farming_cells: &mut u32,
    prior_sell: &BTreeMap<String, u64>,
    pops: &[PopState],
    coastal: bool,
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
                let output_units = starting_method_output(
                    rules,
                    method,
                    good_id,
                    *base_output,
                    terrain.as_deref(),
                )?;
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
            let produced =
                starting_method_output(rules, method, good_id, *quantity, terrain.as_deref())?;
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
) -> Result<u128, DataError> {
    let suitability = method_suitability_pm(rules, method, good_id, terrain);
    u128::from(base_output)
        .checked_mul(u128::from(suitability))
        .ok_or_else(|| {
            missing_data(
                format!("buildings.methods.{}.annual_output.{good_id}", method.id),
                "annual output estimate overflow",
            )
        })
        .map(|output| output / 1000)
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
fn pay_storehouse_fee(
    rules: &Rules,
    place_id: &str,
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
    /// Display-only monthly event counters.
    pub counters: CounterBuckets,
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
    /// Herd counts when the pass began, by cattle, pigs and sheep.
    pub herds_before: Vec<u64>,
    /// Herd counts after the pass, by cattle, pigs and sheep.
    pub herds_after: Vec<u64>,
    /// Per-place results in scenario order.
    pub places: Vec<PlaceProductionLedger>,
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

/// Run the first-of-month seasonal production phase, committing all places together.
///
/// Production uses the method assignment and terrain suitability saved at startup.
/// Fractional output is retained in each building's canonical carry state.
///
/// # Errors
/// Refuses malformed saved dimensions, missing seasonal data, or checked arithmetic overflow;
/// on refusal, `state` is unchanged.
#[expect(
    clippy::too_many_lines,
    reason = "The first-of-month order keeps production, herd updates, payday and storehouse accounting in one rollback boundary."
)]
pub fn run_monthly_production(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<ProductionLedger, DataError> {
    if date.day != 1 {
        return Ok(ProductionLedger::new(rules.goods.len()));
    }
    rules.validate("economy rules")?;
    let mut next = state.clone();
    let mut ledger = ProductionLedger::new(rules.goods.len());
    for place in &mut next.places {
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
                    let numerator = u128::from(*annual)
                        .checked_mul(suitability)
                        .and_then(|value| value.checked_mul(month_share))
                        .and_then(|value| value.checked_mul(u128::from(method_staffed)))
                        .and_then(|value| value.checked_mul(u128::from(coverage_efficiency)))
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
            }
            place.prices_milli.encode(out)?;
            encode_counters(&place.counters, out)?;
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
                });
            }
            places.push(PlaceState {
                id,
                pops,
                buildings,
                prices_milli: Vec::<u64>::decode(input)?,
                counters: decode_counters(input)?,
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
            counters: CounterBuckets::default(),
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
                    },
                ],
                prices_milli: vec![0; goods],
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
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            herds: Vec::new(),
            employment: vec![Employment::default(); 2],
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
                counters: CounterBuckets::default(),
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
                    },
                ],
                prices_milli: vec![100_000],
            }]
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
                    usable: BTreeMap::from([("pasture".to_owned(), 4)]),
                    sea: 0,
                },
            }],
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
                    usable: BTreeMap::from([("pasture".to_owned(), 4)]),
                    sea: 0,
                },
            }],
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
        defaults.storehouse_levels = 1;
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
            starting_method_output(&rules, method, "beef", 1100, Some("pasture")),
            Ok(825)
        );
        assert_eq!(
            starting_method_output(&rules, method, "food", 300, Some("pasture")),
            Ok(300)
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
    #[expect(
        clippy::too_many_lines,
        reason = "The round-trip test encodes one carries/accounts/herds state and compares its next month before and after load."
    )]
    fn canonical_economy_round_trip_is_byte_stable() {
        let state = EconomyState {
            places: vec![PlaceState {
                id: "a".to_owned(),
                counters: CounterBuckets::default(),
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
                    },
                ],
                prices_milli: vec![120_000],
            }],
        };
        let bytes = state.to_bytes().unwrap();
        let decoded = EconomyState::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, state);
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
                    usable: BTreeMap::from([("pasture".to_owned(), 2)]),
                    sea: 0,
                },
            }],
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

        let pop = |workers, employment| PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            workers,
            dependents: 0,
            employment,
            stock: vec![0],
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
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    workers: 0,
                    dependents: 0,
                    employment: vec![Employment::default()],
                    stock: vec![0; rules.goods.len()],
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
                }],
                prices_milli: vec![0; rules.goods.len()],
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
}
