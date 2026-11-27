use super::bigint::Big;
use std::cmp::Ordering;

/// A nonnegative exact quantity, reduced to lowest terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fraction {
    /// Numerator in lowest terms.
    pub numerator: Big,
    /// Positive denominator in lowest terms.
    pub denominator: Big,
}
impl Fraction {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Callers construct these integers for reduction and hand ownership to the fraction."
    )]
    fn new(numerator: Big, denominator: Big) -> Self {
        assert_ne!(denominator, Big::default());
        // Most quantities fit 128 bits, where the reduction is a few machine divisions.
        if let (Some(n), Some(d)) = (numerator.to_u128(), denominator.to_u128()) {
            let (mut a, mut b) = (n, d);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            return Self {
                numerator: Big::new(n / a),
                denominator: Big::new(d / a),
            };
        }
        let mut a = numerator.clone();
        let mut b = denominator.clone();
        while b != Big::default() {
            let r = a.divmod(&b).1;
            a = b;
            b = r;
        }
        Self {
            numerator: numerator.divmod(&a).0,
            denominator: denominator.divmod(&a).0,
        }
    }
    fn integer(n: u128) -> Self {
        Self {
            numerator: Big::new(n),
            denominator: Big::new(1),
        }
    }
    fn ratio(n: u128, d: u128) -> Self {
        Self::new(Big::new(n), Big::new(d))
    }
    fn add(&self, other: &Self) -> Self {
        Self::new(
            self.numerator
                .mul(&other.denominator)
                .add(&other.numerator.mul(&self.denominator)),
            self.denominator.mul(&other.denominator),
        )
    }
    fn sub(&self, other: &Self) -> Option<Self> {
        self.numerator
            .mul(&other.denominator)
            .sub(&other.numerator.mul(&self.denominator))
            .map(|n| Self::new(n, self.denominator.mul(&other.denominator)))
    }
    fn mul(&self, other: &Self) -> Self {
        Self::new(
            self.numerator.mul(&other.numerator),
            self.denominator.mul(&other.denominator),
        )
    }
    fn div(&self, other: &Self) -> Self {
        Self::new(
            self.numerator.mul(&other.denominator),
            self.denominator.mul(&other.numerator),
        )
    }
    fn cmp(&self, other: &Self) -> Ordering {
        self.numerator
            .mul(&other.denominator)
            .cmp(&other.numerator.mul(&self.denominator))
    }
    fn min(&self, other: &Self) -> Self {
        if self.cmp(other) == Ordering::Greater {
            other.clone()
        } else {
            self.clone()
        }
    }
    fn saturating_sub(&self, other: &Self) -> Self {
        self.sub(other).unwrap_or_else(|| Self::integer(0))
    }
}

/// An exact signed difference, with a single representation of zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedFraction {
    /// Whether the value is below zero.
    pub negative: bool,
    /// Absolute value.
    pub magnitude: Fraction,
}
impl SignedFraction {
    fn difference(a: &Fraction, b: &Fraction) -> Self {
        if let Some(magnitude) = a.sub(b) {
            Self {
                negative: false,
                magnitude,
            }
        } else {
            Self {
                negative: true,
                magnitude: b.sub(a).expect("ordered"),
            }
        }
    }
    fn positive(&self) -> bool {
        !self.negative && self.magnitude.numerator != Big::default()
    }
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => other.magnitude.cmp(&self.magnitude),
            (false, false) => self.magnitude.cmp(&other.magnitude),
        }
    }
}

use super::{
    BuildingState, BuildingType, DataError, Date, LandCounts, LevelRecord, Method, PlaceState,
    PopState, Rules,
};
use super::{
    add_counter_units, building_job_pools, choose_terrain, derive_level_sums, give, good_staffing,
    herd_supplies, manager_factor, mean_fertility, method_suitability_pm, missing_data,
    mul_div_floor, pool_staffing, projected_pop_order_units, seasonal_accounts, settle_overflow,
    split_and_pay,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
/// One possible move and its exact score.
pub struct MoveLook {
    /// Stable level number.
    pub level: u32,
    /// Target method index.
    pub method: usize,
    /// Target ground, or none at sea.
    pub ground: Option<String>,
    /// Multiple-source or multi-level transition, when not a one-for-one move.
    pub plan: Option<MovePlan>,
    /// Value of the current method.
    pub staying: Fraction,
    /// Value of the alternative.
    pub switching: Fraction,
    /// Value of work lost.
    pub unpaid: Fraction,
    /// Value lost during the first year.
    pub slow_start: Fraction,
    /// Net loss of animals or founder cost.
    pub herd_cost: Fraction,
    /// Gain after losses and the margin.
    pub surplus: SignedFraction,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// The sources removed and destination grounds created by a capacity move.
pub struct MovePlan {
    /// Existing record numbers to remove.
    pub sources: Vec<u32>,
    /// Ground for each newly created level.
    pub grounds: Vec<Option<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// Every tried move and the index of the best passing move, if any.
pub struct Look {
    /// Moves tried, in tie-break order.
    pub moves: Vec<MoveLook>,
    /// Index of the best passing move.
    pub answer: Option<usize>,
}

fn definition<'a>(
    rules: &'a Rules,
    building: &BuildingState,
) -> Result<&'a BuildingType, DataError> {
    rules
        .buildings
        .iter()
        .find(|item| item.id == building.kind)
        .ok_or_else(|| missing_data(format!("buildings.{}", building.kind), "definition missing"))
}
fn staffing(
    place: &PlaceState,
    building: &BuildingState,
    def: &BuildingType,
) -> Result<(u64, u64, u64), DataError> {
    let positions = u64::from(building.levels);
    let workers = building
        .employment
        .iter()
        .try_fold(0_u64, |sum, entry| sum.checked_add(entry.workers))
        .ok_or_else(|| missing_data("employment".into(), "worker count overflow"))?;
    let efficiency = manager_factor(def, building, &vec![0; place.pops.len()], 1)?;
    Ok((workers, positions, efficiency))
}
fn amount(n: &[u128], d: &[u128]) -> Fraction {
    Fraction::new(super::bigint::product(n), super::bigint::product(d))
}
fn forecast_staffing(
    def: &BuildingType,
    building: &BuildingState,
    method: &Method,
    good: &str,
    levels: &[u32],
) -> Result<(u64, u64), DataError> {
    let current = building_job_pools(def, &building.method_levels)?;
    let future = building_job_pools(def, levels)?;
    let staffed = pool_staffing(
        &current,
        &building.employment,
        &vec![0; building.employment.len()],
        1,
    )?;
    if let Some(id) = method.made_by.get(good) {
        if let Some(target) = future.iter().find(|pool| pool.job.id == *id)
            && let Some(index) = current.iter().position(|pool| pool.job.id == *id)
        {
            return Ok((staffed[index].min(target.places), target.places));
        }
    } else {
        let capacity = future
            .iter()
            .filter(|pool| !pool.job.manager)
            .try_fold(0_u64, |sum, pool| sum.checked_add(pool.places))
            .ok_or_else(|| missing_data("look.jobs".into(), "capacity overflow"))?;
        let filled = current
            .iter()
            .zip(&staffed)
            .filter(|(pool, _)| !pool.job.manager)
            .try_fold(0_u64, |sum, (_, count)| sum.checked_add(*count))
            .ok_or_else(|| missing_data("look.jobs".into(), "staffing overflow"))?;
        return Ok((filled.min(capacity), capacity));
    }
    let ordinary = current
        .iter()
        .zip(&staffed)
        .filter(|(pool, _)| !pool.job.manager)
        .try_fold((0_u64, 0_u64), |(filled, places), (pool, count)| {
            Some((
                filled.checked_add(*count)?,
                places.checked_add(pool.places)?,
            ))
        })
        .ok_or_else(|| missing_data("look.jobs".into(), "staffing overflow"))?;
    Ok(ordinary)
}

// Forecast only hires into jobs new to the building; transfers themselves are exact.
fn projected_staffing(
    place: &PlaceState,
    building: &BuildingState,
    def: &BuildingType,
    levels: &[u32],
) -> Result<(Vec<super::JobPool>, Vec<Fraction>, Fraction), DataError> {
    let before = building_job_pools(def, &building.method_levels)?;
    let current = pool_staffing(
        &before,
        &building.employment,
        &vec![0; building.employment.len()],
        1,
    )?;
    let (filled, places) = before
        .iter()
        .zip(&current)
        .filter(|(p, _)| !p.job.manager)
        .try_fold((0_u64, 0_u64), |(f, p), (pool, count)| {
            Some((f.checked_add(*count)?, p.checked_add(pool.places)?))
        })
        .ok_or_else(|| missing_data("look.jobs".into(), "staffing overflow"))?;
    let mut exact = building.clone();
    exact.method_levels = levels.to_vec();
    let mut pops = place.pops.clone();
    trim_jobs(&mut pops, &mut exact, def, 0)?;
    let pools = building_job_pools(def, levels)?;
    let counts = pool_staffing(
        &pools,
        &exact.employment,
        &vec![0; exact.employment.len()],
        1,
    )?;
    let mut forecast = pools
        .iter()
        .zip(&counts)
        .map(|(pool, &count)| {
            let actual = Fraction::integer(u128::from(count));
            if places > 0 && !before.iter().any(|old| old.job.id == pool.job.id) {
                actual.add(&Fraction::ratio(
                    u128::from(pool.places - count) * u128::from(filled),
                    u128::from(places),
                ))
            } else {
                actual
            }
        })
        .collect::<Vec<_>>();
    if places == 0 {
        let mut unemployed = place
            .pops
            .iter()
            .enumerate()
            .map(|(index, pop)| {
                let all = pop
                    .employment
                    .iter()
                    .fold(0_u64, |sum, entry| sum.saturating_add(entry.workers));
                let old = building
                    .employment
                    .get(index)
                    .map_or(0, |entry| entry.workers);
                let after = exact.employment.get(index).map_or(0, |entry| entry.workers);
                pop.workers
                    .saturating_sub(all.saturating_sub(old).saturating_add(after))
            })
            .collect::<Vec<_>>();
        for (index, pool) in pools.iter().enumerate() {
            let mut open = pool.places.saturating_sub(counts[index]);
            for kind in &pool.job.pop_types {
                for (pop_index, pop) in place.pops.iter().enumerate() {
                    if &pop.kind == kind {
                        let take = open.min(unemployed[pop_index]);
                        unemployed[pop_index] -= take;
                        open -= take;
                        forecast[index] = forecast[index].add(&Fraction::integer(u128::from(take)));
                    }
                }
            }
        }
    }
    let managers = pools
        .iter()
        .zip(&forecast)
        .filter(|(pool, _)| pool.job.manager)
        .fold(Fraction::integer(0), |sum, (_, count)| sum.add(count));
    let total_levels = levels.iter().map(|&count| u64::from(count)).sum::<u64>();
    let factor = if pools.iter().any(|pool| pool.job.manager) && total_levels > 0 {
        let missing = Fraction::integer(u128::from(total_levels)).saturating_sub(&managers);
        let penalty = missing.mul(&Fraction::ratio(
            u128::from(def.penalty_pm),
            u128::from(total_levels),
        ));
        let rounded = penalty.numerator.divmod(&penalty.denominator).0;
        Fraction::integer(1000).saturating_sub(&Fraction::new(rounded, Big::new(1)))
    } else {
        Fraction::integer(1000)
    };
    Ok((pools, forecast, factor))
}

#[expect(
    clippy::too_many_arguments,
    reason = "The formula needs the place, level and alternative at a stated moment."
)]
fn output(
    rules: &Rules,
    place: &PlaceState,
    building: &BuildingState,
    def: &BuildingType,
    method: &Method,
    ground: Option<&str>,
    paydays: u8,
    month: u8,
    quality: u16,
    full: bool,
    candidate_levels: Option<&[u32]>,
) -> Result<Vec<Fraction>, DataError> {
    let mut values = vec![Fraction::integer(0); rules.goods.len()];
    let (_, _, efficiency) = staffing(place, building, def)?;
    let projection = candidate_levels
        .map(|levels| projected_staffing(place, building, def, levels))
        .transpose()?;
    for (good, annual) in &method.annual_output {
        let (workers, positions, factor) = if let Some((pools, forecast, factor)) = &projection {
            let selected = pools
                .iter()
                .zip(forecast)
                .filter(|(pool, _)| {
                    method
                        .made_by
                        .get(good)
                        .map_or(!pool.job.manager, |id| *id == pool.job.id)
                })
                .collect::<Vec<_>>();
            let workers = selected
                .iter()
                .fold(Fraction::integer(0), |sum, (_, count)| sum.add(count));
            let places = selected
                .iter()
                .try_fold(0_u64, |sum, (pool, _)| sum.checked_add(pool.places))
                .ok_or_else(|| missing_data("look.jobs".into(), "capacity overflow"))?;
            (workers, places, factor.clone())
        } else {
            let (workers, places) =
                forecast_staffing(def, building, method, good, &building.method_levels)?;
            (
                Fraction::integer(u128::from(workers)),
                places,
                Fraction::integer(u128::from(efficiency)),
            )
        };
        if positions == 0 {
            continue;
        }
        if herd_supplies(rules, def, good) {
            continue;
        }
        let index = rules
            .goods
            .iter()
            .position(|g| g.id == *good)
            .ok_or_else(|| missing_data(format!("goods.{good}"), "missing good"))?;
        let key = if ["oats", "barley", "peas_beans"].contains(&good.as_str()) {
            "grain"
        } else {
            good.as_str()
        };
        let season = rules
            .seasons
            .get(key)
            .ok_or_else(|| missing_data(format!("seasons.{key}"), "season missing"))?;
        let mut seasonal_weight = 0u128;
        for next in 1..=12u8 {
            let count = if full {
                13
            } else {
                paydays.saturating_add(next).min(13)
            };
            let m = usize::from((month - 1 + next) % 12);
            seasonal_weight += u128::from(count - 1) * u128::from(season[m]);
        }
        let suitability = method_suitability_pm(rules, def, method, good, ground);
        let (q, reference) = if method.resource == "usable_land" {
            (quality, rules.fertility_reference_pm)
        } else {
            (1, 1)
        };
        values[index] = amount(
            &[
                u128::from(*annual),
                u128::from(suitability),
                1,
                1,
                u128::from(q),
                seasonal_weight,
            ],
            &[
                1000,
                u128::from(positions),
                1000,
                u128::from(reference),
                12,
                1000,
            ],
        )
        .mul(&workers)
        .mul(&factor);
    }
    Ok(values)
}
fn herd_output(
    rules: &Rules,
    place: &PlaceState,
    building: &BuildingState,
    def: &BuildingType,
    heads: &[u64],
    candidate_factor: Option<&Fraction>,
) -> Result<Vec<Fraction>, DataError> {
    let mut result = vec![Fraction::integer(0); rules.goods.len()];
    let (_, _, coverage) = staffing(place, building, def)?;
    let factor = candidate_factor
        .cloned()
        .unwrap_or_else(|| Fraction::integer(u128::from(coverage)));
    for (species, head) in rules.herd_species.iter().zip(heads) {
        if !def.herds.contains_key(&species.animal) {
            continue;
        }
        for product in &species.products {
            let i = rules
                .goods
                .iter()
                .position(|g| g.id == product.good)
                .ok_or_else(|| missing_data(format!("goods.{}", product.good), "missing good"))?;
            let value = amount(
                &[
                    u128::from(*head),
                    u128::from(product.amount_milli.unwrap_or(0)),
                    1,
                ],
                &[1000, 1000],
            )
            .mul(&factor);
            result[i] = result[i].add(&value);
        }
    }
    Ok(result)
}

#[derive(Clone)]
struct Wanted {
    goods: Vec<Fraction>,
    jobs: Vec<Fraction>,
    heat: Fraction,
}
/// What a place wants before anyone's output is counted, and the output of every level and herd it has.
struct WantedBase {
    goods: Vec<Fraction>,
    shortages: Vec<Fraction>,
    heat: Fraction,
    produced: Vec<Fraction>,
}
fn wanted_base(
    rules: &Rules,
    place: &PlaceState,
    jobs: &BTreeMap<String, u64>,
    month: u8,
    quality: u16,
) -> Result<WantedBase, DataError> {
    let zero = Fraction::integer(0);
    let mut goods = vec![zero.clone(); rules.goods.len()];
    let mut shortages = vec![zero.clone(); rules.goods.len()];
    let mut heat = zero.clone();
    for pop in &place.pops {
        heat = heat.add(&amount(
            &[super::fuel::fuel_year_target(rules, place, pop)?],
            &[1000],
        ));
        for (i, good) in rules.goods.iter().enumerate() {
            if good.category_supplied("fuel", 1) == 0 {
                goods[i] = goods[i].add(&Fraction::integer(projected_pop_order_units(
                    rules, pop, i,
                )?));
            }
        }
    }
    for (i, good) in rules.goods.iter().enumerate() {
        let job = Fraction::integer(u128::from(jobs.get(&good.id).copied().unwrap_or(0)));
        if good.category_supplied("fuel", 1) > 0 {
            shortages[i] = job;
        } else {
            goods[i] = goods[i].add(&job);
        }
    }
    let mut produced = vec![zero; rules.goods.len()];
    for building in &place.buildings {
        let def = definition(rules, building)?;
        for record in &building.level_records {
            let method = &def.methods[record.method as usize];
            let made = output(
                rules,
                place,
                building,
                def,
                method,
                record.ground.as_deref(),
                record.paydays,
                month,
                quality,
                false,
                None,
            )?;
            for (total, item) in produced.iter_mut().zip(made) {
                *total = total.add(&item);
            }
        }
        let herd = herd_output(rules, place, building, def, &building.herds, None)?;
        for (total, item) in produced.iter_mut().zip(herd) {
            *total = total.add(&item);
        }
    }
    Ok(WantedBase {
        goods,
        shortages,
        heat,
        produced,
    })
}
/// What the place wants of each good, of each job's fuel and of heat once everyone's output but `own` (the output of the level
/// that is tried and of its building's herd) is counted.
fn wanted_without(rules: &Rules, base: &WantedBase, own: &[Fraction]) -> Wanted {
    let mut goods = base.goods.clone();
    let mut shortages = base.shortages.clone();
    let mut heat = base.heat.clone();
    let produced: Vec<Fraction> = base
        .produced
        .iter()
        .zip(own)
        .map(|(total, mine)| total.saturating_sub(mine))
        .collect();
    for (i, good) in rules.goods.iter().enumerate() {
        let fuel = good.category_supplied("fuel", 1);
        if fuel > 0 {
            let used = produced[i].min(&shortages[i]);
            shortages[i] = shortages[i].saturating_sub(&used);
            let rest = produced[i].saturating_sub(&used);
            heat = heat.saturating_sub(&rest.mul(&Fraction::ratio(fuel, 1000)));
        } else {
            goods[i] = goods[i].saturating_sub(&produced[i]);
        }
    }
    Wanted {
        goods,
        jobs: shortages,
        heat,
    }
}
/// The output of one level and of its building's herd, which the want leaves out when that level is tried.
fn own_output(
    rules: &Rules,
    place: &PlaceState,
    building: &BuildingState,
    def: &BuildingType,
    record: &LevelRecord,
    month: u8,
    quality: u16,
) -> Result<Vec<Fraction>, DataError> {
    let mut own = output(
        rules,
        place,
        building,
        def,
        &def.methods[record.method as usize],
        record.ground.as_deref(),
        record.paydays,
        month,
        quality,
        false,
        None,
    )?;
    for (total, item) in own.iter_mut().zip(herd_output(
        rules,
        place,
        building,
        def,
        &building.herds,
        None,
    )?) {
        *total = total.add(&item);
    }
    Ok(own)
}
#[cfg(test)]
fn wanted(
    rules: &Rules,
    place: &PlaceState,
    jobs: &BTreeMap<String, u64>,
    excluded: usize,
    level: u32,
    month: u8,
    quality: u16,
) -> Result<Wanted, DataError> {
    let base = wanted_base(rules, place, jobs, month, quality)?;
    let mut own = vec![Fraction::integer(0); rules.goods.len()];
    if let Some(building) = place.buildings.get(excluded) {
        let def = definition(rules, building)?;
        if let Some(record) = building.level_records.iter().find(|r| r.number == level) {
            own = own_output(rules, place, building, def, record, month, quality)?;
        } else {
            // A level that is not there takes only its building's herd with it.
            for (total, item) in own.iter_mut().zip(herd_output(
                rules,
                place,
                building,
                def,
                &building.herds,
                None,
            )?) {
                *total = total.add(&item);
            }
        }
    }
    Ok(wanted_without(rules, &base, &own))
}
fn value(rules: &Rules, quantities: &[Fraction], want: &Wanted, prices: &[u64]) -> Fraction {
    let mut total = Fraction::integer(0);
    let mut heat = want.heat.clone();
    for (i, good) in rules.goods.iter().enumerate() {
        let mut count;
        let fuel = good.category_supplied("fuel", 1);
        if fuel > 0 {
            let job = quantities[i].min(&want.jobs[i]);
            let rest = quantities[i].saturating_sub(&job);
            let supply = Fraction::ratio(fuel, 1000);
            let for_heat = rest.min(&heat.div(&supply));
            heat = heat.saturating_sub(&for_heat.mul(&supply));
            count = job.add(&for_heat);
        } else {
            count = quantities[i].min(&want.goods[i]);
        }
        count = count.mul(&Fraction::integer(u128::from(prices[i])));
        total = total.add(&count);
    }
    total
}
/// What the place's levels use, counted once for a look: the field room each ground gives up to usable-land levels (in
/// thousandths) and the cells each terrain gives up to forest and bog levels, whatever their methods are called. A level that is
/// tried leaves out its own.
#[derive(Clone)]
struct Usage {
    by_ground: BTreeMap<String, u64>,
    cells: BTreeMap<String, u64>,
}
/// The cells a forest or bog level takes.
fn cells_of(method: &Method) -> u64 {
    u64::from(method.cells_per_level.unwrap_or(1))
}
impl Usage {
    fn new(rules: &Rules, place: &PlaceState) -> Result<Self, DataError> {
        let mut by_ground = BTreeMap::<String, u64>::new();
        let mut cells = BTreeMap::<String, u64>::new();
        for building in &place.buildings {
            let def = definition(rules, building)?;
            for record in &building.level_records {
                let method = &def.methods[record.method as usize];
                if method.resource == "forest" || method.resource == "bog" {
                    *cells.entry(method.resource.clone()).or_default() += cells_of(method);
                }
                if method.resource == "usable_land"
                    && let Some(ground) = &record.ground
                {
                    *by_ground.entry(ground.clone()).or_default() +=
                        u64::from(def.land_per_level) * 1000;
                }
            }
        }
        Ok(Self { by_ground, cells })
    }
}
/// Choose each destination after releasing all sources and reserving each earlier destination.
fn capacity_grounds(
    rules: &Rules,
    def: &BuildingType,
    land: &LandCounts,
    usage: &Usage,
    sources: &[&LevelRecord],
    target: &Method,
    count: u64,
) -> Vec<Option<String>> {
    let mut usage = usage.clone();
    for record in sources {
        let method = &def.methods[record.method as usize];
        if method.resource == "usable_land" {
            if let Some(ground) = &record.ground {
                let used = usage.by_ground.entry(ground.clone()).or_default();
                *used = used.saturating_sub(u64::from(def.land_per_level) * 1000);
            }
        } else if ["forest", "bog"].contains(&method.resource.as_str()) {
            let used = usage.cells.entry(method.resource.clone()).or_default();
            *used = used.saturating_sub(cells_of(method));
        }
    }
    let mut grounds = Vec::new();
    for _ in 0..count {
        let ground = match target.resource.as_str() {
            "none" => None,
            "sea" if land.sea > 0 => None,
            "forest" | "bog" => {
                let used = usage.cells.entry(target.resource.clone()).or_default();
                let need = cells_of(target);
                if used.saturating_add(need)
                    > u64::from(land.land_cells.get(&target.resource).copied().unwrap_or(0))
                {
                    break;
                }
                *used += need;
                Some(target.resource.clone())
            }
            "usable_land" => {
                let mut available = land.clone();
                for (ground, used) in &usage.by_ground {
                    let room = available.fertility_pm.entry(ground.clone()).or_default();
                    *room = room.saturating_sub(*used);
                }
                let Some(ground) = choose_terrain(
                    rules,
                    target,
                    &available,
                    u64::from(def.land_per_level) * 1000,
                ) else {
                    break;
                };
                *usage.by_ground.entry(ground.clone()).or_default() +=
                    u64::from(def.land_per_level) * 1000;
                Some(ground)
            }
            _ => break,
        };
        grounds.push(ground);
    }
    grounds
}

fn free_capacity(def: &BuildingType, building: &BuildingState) -> Result<u64, DataError> {
    if def.methods.len() != building.method_levels.len() {
        return Err(missing_data(
            "switch.capacity".into(),
            "method counts do not match",
        ));
    }
    let used = def
        .methods
        .iter()
        .zip(&building.method_levels)
        .try_fold(0_u64, |sum, (method, levels)| {
            sum.checked_add(u64::from(method.size_units) * u64::from(*levels))
        })
        .ok_or_else(|| missing_data("switch.capacity".into(), "capacity overflow"))?;
    u64::from(building.units)
        .checked_sub(used)
        .ok_or_else(|| missing_data("switch.capacity".into(), "used capacity exceeds units"))
}

#[expect(
    clippy::option_option,
    reason = "Outer none means no room; inner none means a valid sea method."
)]
fn target_ground(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    usage: &Usage,
    building_index: usize,
    level: &LevelRecord,
    method_index: usize,
) -> Result<Option<Option<String>>, DataError> {
    let building = &place.buildings[building_index];
    let def = definition(rules, building)?;
    let method = &def.methods[method_index];
    if method.resource == "none" {
        return Ok(Some(None));
    }
    if method.resource == "sea" {
        return Ok((land.sea > 0).then_some(None));
    }
    let own_method = &def.methods[level.method as usize];
    if method.resource == "forest" || method.resource == "bog" {
        // The terrain's cells are shared by every method that takes them, whatever the method is called: the other levels' cells
        // are taken off, and the tried level gives back its own.
        let used = usage.cells.get(&method.resource).copied().unwrap_or(0)
            - if own_method.resource == method.resource {
                cells_of(own_method)
            } else {
                0
            };
        let supply = u64::from(land.land_cells.get(&method.resource).copied().unwrap_or(0));
        return Ok((used + cells_of(method) <= supply).then_some(Some(method.resource.clone())));
    }
    // The room every other usable-land level leaves: each ground's fertility less what they use, and no less than nothing.
    let mut remaining = land.clone();
    for (ground, used) in &usage.by_ground {
        let own = if own_method.resource == "usable_land" && level.ground.as_ref() == Some(ground) {
            u64::from(def.land_per_level) * 1000
        } else {
            0
        };
        let available = remaining.fertility_pm.entry(ground.clone()).or_default();
        *available = available.saturating_sub(used - own);
    }
    Ok(choose_terrain(
        rules,
        method,
        &remaining,
        u64::from(def.land_per_level) * 1000,
    )
    .map(Some))
}
fn rescale_carries(
    rules: &Rules,
    building: &mut BuildingState,
    definition: &BuildingType,
    levels: &[u32],
) -> Result<(), DataError> {
    let old = building_job_pools(definition, &building.method_levels)?;
    let new = building_job_pools(definition, levels)?;
    let old_zero = vec![0; old.len()];
    let new_zero = vec![0; new.len()];
    for (method_index, row) in building.production_carries.iter_mut().enumerate() {
        let method = definition.methods.get(method_index).ok_or_else(|| {
            missing_data(
                "production.carries".into(),
                "method dimensions do not match",
            )
        })?;
        for (index, carry) in row.iter_mut().enumerate() {
            let good = rules.goods.get(index).ok_or_else(|| {
                missing_data("production.carries".into(), "good dimensions do not match")
            })?;
            let (_, before) = good_staffing(method, &good.id, &old, &old_zero, 1)?;
            let (_, after) = good_staffing(method, &good.id, &new, &new_zero, 1)?;
            *carry = if before == 0 || after == 0 {
                0
            } else {
                mul_div_floor(*carry, u128::from(after), u128::from(before)).ok_or_else(|| {
                    missing_data("production.carries".into(), "carry rescale overflow")
                })?
            };
        }
    }
    Ok(())
}

fn trim_jobs(
    pops: &mut [PopState],
    building: &mut BuildingState,
    definition: &BuildingType,
    building_index: usize,
) -> Result<(), DataError> {
    let pools = building_job_pools(definition, &building.method_levels)?;
    if building.employment.len() != pops.len() {
        return Err(missing_data(
            "employment.jobs".into(),
            "employment dimensions do not match",
        ));
    }
    let mut released = vec![0_u64; pops.len()];
    let ids = building
        .employment
        .iter()
        .flat_map(|entry| entry.jobs.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    for id in ids {
        let counts = building
            .employment
            .iter()
            .map(|entry| entry.jobs.get(&id).copied().unwrap_or(0))
            .collect::<Vec<_>>();
        let total = counts
            .iter()
            .try_fold(0_u64, |sum, count| sum.checked_add(*count))
            .ok_or_else(|| missing_data("employment.jobs".into(), "job worker total overflow"))?;
        let places = pools
            .iter()
            .find(|pool| pool.job.id == id)
            .map_or(0, |pool| pool.places);
        let excess = give(total.saturating_sub(places), &counts)?;
        for (i, entry) in building.employment.iter_mut().enumerate() {
            released[i] = released[i]
                .checked_add(excess[i])
                .ok_or_else(|| missing_data("employment.jobs".into(), "release overflow"))?;
            let remaining = counts[i] - excess[i];
            if remaining == 0 {
                entry.jobs.remove(&id);
            } else {
                entry.jobs.insert(id.clone(), remaining);
            }
        }
    }
    for pool in &pools {
        let occupied = building
            .employment
            .iter()
            .map(|entry| entry.jobs.get(&pool.job.id).copied().unwrap_or(0))
            .try_fold(0_u64, u64::checked_add)
            .ok_or_else(|| missing_data("employment.jobs".into(), "job worker total overflow"))?;
        let eligible = released
            .iter()
            .zip(pops.iter())
            .map(|(&count, pop)| {
                if pool.job.pop_types.contains(&pop.kind) {
                    count
                } else {
                    0
                }
            })
            .collect::<Vec<_>>();
        let awards = give(pool.places.saturating_sub(occupied), &eligible)?;
        for (i, award) in awards.into_iter().enumerate() {
            if award > 0 {
                *building.employment[i]
                    .jobs
                    .entry(pool.job.id.clone())
                    .or_default() += award;
                released[i] -= award;
            }
        }
    }
    for (pop, entry) in pops.iter_mut().zip(&mut building.employment) {
        entry
            .jobs
            .retain(|id, _| pools.iter().any(|pool| pool.job.id == *id));
        entry.workers = entry
            .jobs
            .values()
            .try_fold(0_u64, |sum, count| sum.checked_add(*count))
            .ok_or_else(|| missing_data("employment.jobs".into(), "worker count overflow"))?;
        pop.employment[building_index] = entry.clone();
    }
    Ok(())
}

/// Replace a set of levels and rebuild the pooled jobs once for the complete move.
#[expect(
    clippy::too_many_arguments,
    reason = "The transition needs its source, destination, staffing and date together."
)]
fn replace_levels(
    rules: &Rules,
    pops: &mut [PopState],
    building: &mut BuildingState,
    def: &BuildingType,
    building_index: usize,
    sources: &[u32],
    target_index: usize,
    grounds: &[Option<String>],
    year: u16,
    month: u8,
) -> Result<(), DataError> {
    let target = def
        .methods
        .get(target_index)
        .ok_or_else(|| missing_data("switch.method".into(), "unknown method"))?;
    if sources.iter().any(|number| {
        !building
            .level_records
            .iter()
            .any(|record| record.number == *number)
    }) {
        return Err(missing_data(
            "switch.level".into(),
            "a source level has no record",
        ));
    }
    let insertion = building
        .level_records
        .iter()
        .position(|record| sources.contains(&record.number))
        .unwrap_or(building.level_records.len());
    building
        .level_records
        .retain(|record| !sources.contains(&record.number));
    for (offset, ground) in grounds.iter().enumerate() {
        building.level_records.insert(
            insertion + offset,
            LevelRecord {
                number: 0,
                method: u32::try_from(target_index).map_err(|_| {
                    missing_data("switch.method".into(), "method index exceeds u32")
                })?,
                ground: ground.clone(),
                paydays: if target.grows { 0 } else { 13 },
                unpaid_work: seasonal_accounts(rules, def, target),
                last_switch: Some((year, month)),
            },
        );
    }
    for (index, record) in building.level_records.iter_mut().enumerate() {
        record.number = u32::try_from(index)
            .map_err(|_| missing_data("switch.level".into(), "level count exceeds u32"))?;
    }
    building.levels = u32::try_from(building.level_records.len())
        .map_err(|_| missing_data("switch.level".into(), "level count exceeds u32"))?;
    let mut counts = vec![0_u32; def.methods.len()];
    for record in &building.level_records {
        counts[record.method as usize] += 1;
    }
    rescale_carries(rules, building, def, &counts)?;
    building.method_levels = counts;
    trim_jobs(pops, building, def, building_index)?;
    let (suitability, grazing) = derive_level_sums(rules, def, &building.level_records)?;
    building.method_suitability = suitability;
    building.herd_grazing_sums = grazing;
    Ok(())
}

type HerdSwitch = (Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>);

fn switching_heads(
    rules: &Rules,
    building: &BuildingState,
    def: &BuildingType,
    level: &LevelRecord,
    target: &Method,
) -> Result<HerdSwitch, DataError> {
    let mut levels = building.method_levels.clone();
    levels[level.method as usize] = levels[level.method as usize].saturating_sub(1);
    if let Some(index) = def.methods.iter().position(|method| method.id == target.id) {
        levels[index] = levels[index].saturating_add(1);
    }
    switching_heads_for(rules, building, def, &levels, is_land(target))
}

fn is_land(method: &Method) -> bool {
    ["usable_land", "forest", "bog"].contains(&method.resource.as_str())
}

fn switching_heads_for(
    rules: &Rules,
    building: &BuildingState,
    def: &BuildingType,
    levels: &[u32],
    creates_land: bool,
) -> Result<HerdSwitch, DataError> {
    let keeping = def
        .methods
        .iter()
        .zip(levels)
        .filter(|(method, _)| is_land(method))
        .fold(0_u64, |sum, (_, levels)| {
            sum.saturating_add(u64::from(*levels))
        });
    let pools = building_job_pools(def, levels)?;
    let herder = pools.iter().find(|pool| pool.job.id == "herder");
    let has_butcher = pools.iter().any(|pool| pool.job.id == "butcher");
    let herders = herder.map_or(0, |pool| {
        building
            .employment
            .iter()
            .fold(0_u64, |sum, entry| {
                sum.saturating_add(entry.jobs.get("herder").copied().unwrap_or(0))
            })
            .min(pool.places)
    });
    let mut next = Vec::new();
    let mut gone = Vec::new();
    let mut founders = Vec::new();
    let mut dead = Vec::new();
    for (i, species) in rules.herd_species.iter().enumerate() {
        let current = building.herds.get(i).copied().unwrap_or(0);
        let land = keeping.saturating_mul(def.herds.get(&species.animal).map_or(0, |c| c.maximum));
        let soft = if herder.is_some() {
            land.min(herders.saturating_mul(species.head_per_herder))
        } else {
            land
        };
        let deaths = current.saturating_sub(land.saturating_mul(2));
        let survivors = current - deaths;
        let slaughter = if has_butcher {
            0
        } else {
            survivors.saturating_sub(soft)
        };
        let founder = if current == 0 && creates_land {
            keeping.saturating_mul(2).min(soft)
        } else {
            0
        };
        next.push(survivors - slaughter + founder);
        gone.push(slaughter);
        founders.push(founder);
        dead.push(deaths);
    }
    Ok((next, gone, founders, dead))
}
fn herd_loss(
    rules: &Rules,
    gone: &[u64],
    founders: &[u64],
    prices: &[u64],
) -> Result<Fraction, DataError> {
    let mut loss = Fraction::integer(0);
    for (i, species) in rules.herd_species.iter().enumerate() {
        let animal = rules
            .goods
            .iter()
            .position(|g| g.id == species.animal)
            .ok_or_else(|| missing_data(format!("goods.{}", species.animal), "missing"))?;
        let live = Fraction::integer(u128::from(prices[animal]));
        let mut yield_value = Fraction::integer(0);
        for item in &species.yields {
            let good = rules
                .goods
                .iter()
                .position(|g| g.id == item.good)
                .ok_or_else(|| missing_data(format!("goods.{}", item.good), "missing"))?;
            yield_value = yield_value.add(&amount(
                &[u128::from(item.units), u128::from(prices[good])],
                &[1],
            ));
        }
        loss = loss.add(
            &live
                .saturating_sub(&yield_value)
                .mul(&Fraction::integer(u128::from(gone[i]))),
        );
        loss = loss.add(&live.mul(&Fraction::integer(u128::from(founders[i]))));
    }
    Ok(loss)
}
fn add_quantities(total: &mut [Fraction], values: &[Fraction]) {
    for (total, value) in total.iter_mut().zip(values) {
        *total = total.add(value);
    }
}

fn unpaid_value(
    rules: &Rules,
    record: &LevelRecord,
    prices: &[u64],
) -> Result<Fraction, DataError> {
    let mut total = Fraction::integer(0);
    for (good, account) in &record.unpaid_work {
        let index = rules
            .goods
            .iter()
            .position(|item| item.id == *good)
            .ok_or_else(|| missing_data(format!("goods.{good}"), "missing"))?;
        total = total.add(&Fraction::new(
            account.mul(&Big::new(u128::from(prices[index]))),
            Big::new(144_000),
        ));
    }
    Ok(total)
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "A capacity look evaluates the complete move against one demand and settlement."
)]
fn look_capacity(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    building_index: usize,
    month: u8,
    year: Option<u16>,
    prices: &[u64],
    jobs: &BTreeMap<String, u64>,
    free: u64,
) -> Result<Look, DataError> {
    let building = &place.buildings[building_index];
    let def = definition(rules, building)?;
    let quality = mean_fertility(land)?;
    let base = wanted_base(rules, place, jobs, month, quality)?;
    let usage = Usage::new(rules, place)?;
    let old_herd_made = herd_output(rules, place, building, def, &building.herds, None)?;
    let mut eligible = building
        .level_records
        .iter()
        .filter(|record| {
            year.is_none_or(|year| record.last_switch.is_none_or(|stamp| stamp < (year, month)))
        })
        .collect::<Vec<_>>();
    eligible.sort_by_key(|record| record.number);
    let mut moves = Vec::<MoveLook>::new();
    let mut answer: Option<usize> = None;
    for (target_index, target) in def.methods.iter().enumerate() {
        let target_size = u64::from(target.size_units);
        if target_size == 0 {
            return Err(missing_data("switch.capacity".into(), "zero target size"));
        }
        let mut sets = Vec::<Vec<&LevelRecord>>::new();
        if free >= target_size {
            sets.push(Vec::new());
        }
        for (source_index, source) in def.methods.iter().enumerate() {
            if source_index == target_index || source.size_units == 0 {
                continue;
            }
            let members = eligible
                .iter()
                .copied()
                .filter(|record| record.method as usize == source_index)
                .collect::<Vec<_>>();
            let size = u64::from(source.size_units);
            if free.saturating_add(size) >= target_size {
                sets.extend(members.iter().map(|record| vec![*record]));
            }
            let needed = target_size.saturating_sub(free).div_ceil(size);
            if needed > 1 && needed <= members.len() as u64 {
                let mut ranked = Vec::new();
                for record in members {
                    let own = own_output(rules, place, building, def, record, month, quality)?;
                    let want = wanted_without(rules, &base, &own);
                    let old = output(
                        rules,
                        place,
                        building,
                        def,
                        source,
                        record.ground.as_deref(),
                        record.paydays,
                        month,
                        quality,
                        true,
                        None,
                    )?;
                    ranked.push((
                        record,
                        value(rules, &old, &want, prices)
                            .add(&unpaid_value(rules, record, prices)?),
                    ));
                }
                ranked.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.number.cmp(&b.0.number)));
                let mut selected = ranked
                    .into_iter()
                    .take(usize::try_from(needed).expect("bounded by member count"))
                    .map(|(record, _)| record)
                    .collect::<Vec<_>>();
                selected.sort_by_key(|record| record.number);
                sets.push(selected);
            }
        }
        sets.sort_by_key(|set| (set.len(), set.first().map_or(0, |record| record.number)));
        for sources in sets {
            let released = sources.iter().fold(free, |sum, record| {
                sum.saturating_add(u64::from(def.methods[record.method as usize].size_units))
            });
            let grounds = capacity_grounds(
                rules,
                def,
                land,
                &usage,
                &sources,
                target,
                released / target_size,
            );
            if grounds.is_empty() {
                continue;
            }
            let mut levels = building.method_levels.clone();
            for source in &sources {
                levels[source.method as usize] -= 1;
            }
            levels[target_index] = levels[target_index]
                .checked_add(
                    u32::try_from(grounds.len())
                        .map_err(|_| missing_data("switch.capacity".into(), "too many levels"))?,
                )
                .ok_or_else(|| missing_data("switch.capacity".into(), "level count overflow"))?;
            let mut own = old_herd_made.clone();
            let mut old = vec![Fraction::integer(0); rules.goods.len()];
            let mut unpaid = Fraction::integer(0);
            for source in &sources {
                let method = &def.methods[source.method as usize];
                add_quantities(
                    &mut own,
                    &output(
                        rules,
                        place,
                        building,
                        def,
                        method,
                        source.ground.as_deref(),
                        source.paydays,
                        month,
                        quality,
                        false,
                        None,
                    )?,
                );
                add_quantities(
                    &mut old,
                    &output(
                        rules,
                        place,
                        building,
                        def,
                        method,
                        source.ground.as_deref(),
                        source.paydays,
                        month,
                        quality,
                        true,
                        None,
                    )?,
                );
                unpaid = unpaid.add(&unpaid_value(rules, source, prices)?);
            }
            let want = wanted_without(rules, &base, &own);
            let old_value = value(rules, &old, &want, prices);
            let mut mature = vec![Fraction::integer(0); rules.goods.len()];
            let mut ramped = mature.clone();
            for ground in &grounds {
                add_quantities(
                    &mut mature,
                    &output(
                        rules,
                        place,
                        building,
                        def,
                        target,
                        ground.as_deref(),
                        0,
                        month,
                        quality,
                        true,
                        Some(&levels),
                    )?,
                );
                add_quantities(
                    &mut ramped,
                    &output(
                        rules,
                        place,
                        building,
                        def,
                        target,
                        ground.as_deref(),
                        if target.grows { 0 } else { 13 },
                        month,
                        quality,
                        false,
                        Some(&levels),
                    )?,
                );
            }
            let new_value = value(rules, &mature, &want, prices);
            let mut exact = building.clone();
            exact.method_levels.clone_from(&levels);
            let mut pops = place.pops.clone();
            trim_jobs(&mut pops, &mut exact, def, building_index)?;
            let (heads, gone, founders, dead) =
                switching_heads_for(rules, &exact, def, &levels, is_land(target))?;
            let old_herd = value(rules, &old_herd_made, &want, prices);
            let (_, _, factor) = projected_staffing(place, building, def, &levels)?;
            let new_herd = value(
                rules,
                &herd_output(rules, place, building, def, &heads, Some(&factor))?,
                &want,
                prices,
            );
            let staying = old_value.add(&old_herd.saturating_sub(&new_herd));
            let switching = new_value.add(&new_herd.saturating_sub(&old_herd));
            let slow_start = new_value.saturating_sub(&value(rules, &ramped, &want, prices));
            let lost = gone
                .iter()
                .zip(&dead)
                .map(|(gone, dead)| gone.saturating_add(*dead))
                .collect::<Vec<_>>();
            let herd_cost = herd_loss(rules, &lost, &founders, prices)?;
            let threshold = staying
                .add(&staying.mul(&Fraction::ratio(
                    u128::from(rules.method_switch_margin_pm),
                    1000,
                )))
                .add(&unpaid)
                .add(&slow_start)
                .add(&herd_cost);
            let surplus = SignedFraction::difference(&switching, &threshold);
            let numbers = sources
                .iter()
                .map(|record| record.number)
                .collect::<Vec<_>>();
            let entry = MoveLook {
                level: numbers.first().copied().unwrap_or(0),
                method: target_index,
                ground: grounds[0].clone(),
                plan: Some(MovePlan {
                    sources: numbers,
                    grounds,
                }),
                staying,
                switching,
                unpaid,
                slow_start,
                herd_cost,
                surplus,
            };
            if entry.surplus.positive()
                && answer.is_none_or(|i| entry.surplus.cmp(&moves[i].surplus) == Ordering::Greater)
            {
                answer = Some(moves.len());
            }
            moves.push(entry);
        }
    }
    Ok(Look { moves, answer })
}

/// Calculates one building's best method switch without changing the place. No level is locked: see [`look_in_year`].
///
/// # Errors
/// Returns a data error for invalid inputs or missing rule definitions.
pub fn look(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    building_index: usize,
    month: u8,
    prices: &[u64],
    jobs: &BTreeMap<String, u64>,
) -> Result<Look, DataError> {
    look_in_year(
        rules,
        place,
        land,
        building_index,
        month,
        None,
        prices,
        jobs,
    )
}

/// Calculates one building's best method switch without changing the place, leaving out the levels that switched in `year`: a
/// level that has switched does not switch again in the same calendar year (owner, S6). With no year, no level is locked.
///
/// # Errors
/// Returns a data error for invalid inputs or missing rule definitions.
#[expect(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "Each alternative follows the score and loss terms together in the contract's order, at a stated moment."
)]
pub fn look_in_year(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    building_index: usize,
    month: u8,
    year: Option<u16>,
    prices: &[u64],
    jobs: &BTreeMap<String, u64>,
) -> Result<Look, DataError> {
    if !(1..=12).contains(&month) || prices.len() != rules.goods.len() {
        return Err(missing_data("look".into(), "invalid month or prices"));
    }
    let building = place
        .buildings
        .get(building_index)
        .ok_or_else(|| missing_data("look.building".into(), "unknown index"))?;
    let def = definition(rules, building)?;
    let free = free_capacity(def, building)?;
    if free > 0 || def.methods.iter().any(|method| method.size_units != 5) {
        return look_capacity(
            rules,
            place,
            land,
            building_index,
            month,
            year,
            prices,
            jobs,
            free,
        );
    }
    let quality = mean_fertility(land)?;
    // What the place wants and what everyone makes are the same for every level tried; each level leaves out its own.
    let base = wanted_base(rules, place, jobs, month, quality)?;
    let usage = Usage::new(rules, place)?;
    let old_herd_made = herd_output(rules, place, building, def, &building.herds, None)?;
    let mut per_level: BTreeMap<u32, (Wanted, Vec<Fraction>, Fraction)> = BTreeMap::new();
    let mut moves: Vec<MoveLook> = Vec::new();
    let mut answer: Option<usize> = None;
    let mut ordered_levels: Vec<&LevelRecord> = building
        .level_records
        .iter()
        .filter(|level| {
            year.is_none_or(|year| level.last_switch.is_none_or(|stamp| stamp < (year, month)))
        })
        .collect();
    ordered_levels.sort_by_key(|level| level.number);
    for (target_index, target) in def.methods.iter().enumerate() {
        for level in &ordered_levels {
            if target_index == level.method as usize {
                continue;
            }
            let Some(ground) = target_ground(
                rules,
                place,
                land,
                &usage,
                building_index,
                level,
                target_index,
            )?
            else {
                continue;
            };
            if let std::collections::btree_map::Entry::Vacant(slot) = per_level.entry(level.number)
            {
                let own = own_output(rules, place, building, def, level, month, quality)?;
                let want = wanted_without(rules, &base, &own);
                let old = output(
                    rules,
                    place,
                    building,
                    def,
                    &def.methods[level.method as usize],
                    level.ground.as_deref(),
                    level.paydays,
                    month,
                    quality,
                    true,
                    None,
                )?;
                let old_value = value(rules, &old, &want, prices);
                slot.insert((want, old, old_value));
            }
            let (want, _, old_value) = &per_level[&level.number];
            let mut candidate_levels = building.method_levels.clone();
            candidate_levels[level.method as usize] -= 1;
            candidate_levels[target_index] += 1;
            let new = output(
                rules,
                place,
                building,
                def,
                target,
                ground.as_deref(),
                0,
                month,
                quality,
                true,
                Some(&candidate_levels),
            )?;
            let slow = output(
                rules,
                place,
                building,
                def,
                target,
                ground.as_deref(),
                if target.grows { 0 } else { 13 },
                month,
                quality,
                false,
                Some(&candidate_levels),
            )?;
            let new_value = value(rules, &new, want, prices);
            let mut exact = building.clone();
            exact.method_levels.clone_from(&candidate_levels);
            let mut pops = place.pops.clone();
            trim_jobs(&mut pops, &mut exact, def, building_index)?;
            // The herd calculation derives the candidate levels from the old record;
            // its staffing, however, is the post-transfer staffing.
            exact.method_levels.clone_from(&building.method_levels);
            let (heads, gone, founders, dead) = switching_heads(rules, &exact, def, level, target)?;
            let old_herd = value(rules, &old_herd_made, want, prices);
            let (_, _, candidate_factor) =
                projected_staffing(place, building, def, &candidate_levels)?;
            let new_herd = value(
                rules,
                &herd_output(rules, place, building, def, &heads, Some(&candidate_factor))?,
                want,
                prices,
            );
            let staying = old_value.add(&old_herd.saturating_sub(&new_herd));
            let switching = new_value.add(&new_herd.saturating_sub(&old_herd));
            let mut unpaid = Fraction::integer(0);
            for (good, account) in &level.unpaid_work {
                let i = rules
                    .goods
                    .iter()
                    .position(|g| g.id == *good)
                    .ok_or_else(|| missing_data(format!("goods.{good}"), "missing"))?;
                unpaid = unpaid.add(&Fraction::new(
                    account.mul(&Big::new(u128::from(prices[i]))),
                    Big::new(144_000),
                ));
            }
            let slow_start = new_value.saturating_sub(&value(rules, &slow, want, prices));
            let lost = gone
                .iter()
                .zip(&dead)
                .map(|(slaughter, deaths)| slaughter.saturating_add(*deaths))
                .collect::<Vec<_>>();
            let herd_cost = herd_loss(rules, &lost, &founders, prices)?;
            let threshold = staying
                .add(&staying.mul(&Fraction::ratio(
                    u128::from(rules.method_switch_margin_pm),
                    1000,
                )))
                .add(&unpaid)
                .add(&slow_start)
                .add(&herd_cost);
            let surplus = SignedFraction::difference(&switching, &threshold);
            let entry = MoveLook {
                level: level.number,
                method: target_index,
                ground,
                plan: None,
                staying,
                switching,
                unpaid,
                slow_start,
                herd_cost,
                surplus,
            };
            if entry.surplus.positive()
                && answer.is_none_or(|i| entry.surplus.cmp(&moves[i].surplus) == Ordering::Greater)
            {
                answer = Some(moves.len());
            }
            moves.push(entry);
        }
    }
    Ok(Look { moves, answer })
}

/// Makes the move the look chose for one building, at once: the level changes method and ground and starts over, the building's
/// sums follow its records, and a herd that no longer fits is slaughtered and its yield paid out that day by payday's own split,
/// with no wage posted; where a kind has no animals and the new method keeps animals, founders arrive.
#[expect(
    clippy::too_many_lines,
    reason = "The level, the building's sums, the herd and the cull's payment change together, at once."
)]
fn make_move(
    rules: &Rules,
    place: &mut PlaceState,
    building_index: usize,
    entry: &MoveLook,
    year: u16,
    month: u8,
    ledger: &mut super::PlaceProductionLedger,
) -> Result<(), DataError> {
    let def = definition(rules, &place.buildings[building_index])?;
    let target = &def.methods[entry.method];
    let building = &mut place.buildings[building_index];
    let fallback = MovePlan {
        sources: vec![entry.level],
        grounds: vec![entry.ground.clone()],
    };
    let plan = entry.plan.as_ref().unwrap_or(&fallback);
    replace_levels(
        rules,
        &mut place.pops,
        building,
        def,
        building_index,
        &plan.sources,
        entry.method,
        &plan.grounds,
        year,
        month,
    )?;
    let (_, gone, founders, dead) = switching_heads_for(
        rules,
        building,
        def,
        &building.method_levels,
        is_land(target),
    )?;
    let mut output = vec![0_u64; rules.goods.len()];
    if building.herds.len() == rules.herd_species.len() {
        for (species_index, species) in rules.herd_species.iter().enumerate() {
            let slaughtered = gone[species_index];
            if slaughtered > 0 {
                for item in &species.yields {
                    let Some(good_index) = rules.goods.iter().position(|good| good.id == item.good)
                    else {
                        continue;
                    };
                    let units = slaughtered.checked_mul(item.units).ok_or_else(|| {
                        missing_data(
                            format!("herds.slaughter.{}", item.good),
                            "slaughter yield overflow",
                        )
                    })?;
                    output[good_index] =
                        output[good_index].checked_add(units).ok_or_else(|| {
                            missing_data(
                                format!("herds.slaughter.{}", item.good),
                                "slaughter stock overflow",
                            )
                        })?;
                }
            }
            building.herds[species_index] =
                building.herds[species_index] - slaughtered - dead[species_index]
                    + founders[species_index];
            ledger.herd_deaths[species_index] = ledger.herd_deaths[species_index]
                .checked_add(dead[species_index])
                .ok_or_else(|| missing_data("ledger.herds".into(), "death total overflow"))?;
            ledger.herd_slaughter[species_index] = ledger.herd_slaughter[species_index]
                .checked_add(slaughtered)
                .ok_or_else(|| missing_data("ledger.herds".into(), "slaughter total overflow"))?;
            ledger.herd_arrivals[species_index] = ledger.herd_arrivals[species_index]
                .checked_add(founders[species_index])
                .ok_or_else(|| missing_data("ledger.herds".into(), "arrival total overflow"))?;
        }
    }
    if output.iter().all(|units| *units == 0) {
        return Ok(());
    }
    for (good_index, units) in output.iter().copied().enumerate() {
        add_counter_units(
            &mut place.counters.current.produced,
            rules.goods.len(),
            good_index,
            units,
            "counters.produced",
        )?;
        ledger.produced[good_index] = ledger.produced[good_index]
            .checked_add(units)
            .ok_or_else(|| missing_data("ledger.production".into(), "production total overflow"))?;
    }
    let (_, pending) = split_and_pay(
        rules,
        &place.id,
        &place.prices_milli,
        &mut place.pops,
        &mut place.buildings[building_index],
        def,
        building_index,
        &output,
        // A switch's pay-out is of work already accrued: no day of it is counted away.
        &[],
        1,
    )?;
    for (source, good_index, contributions) in pending {
        settle_overflow(
            rules,
            &mut place.pops,
            &mut place.buildings,
            source,
            good_index,
            &contributions,
            &mut place.counters,
            ledger,
        )?;
    }
    Ok(())
}

/// The month's switching for one place, on the 1st after production, herds and payday. Each building with two or more methods
/// looks in the place's order, at the prices the day began with, and its move is made before the next building looks. No job
/// lacks anything yet.
///
/// # Errors
/// Returns the first data error from a look or a move.
pub(super) fn run_place_switches(
    rules: &Rules,
    place: &mut PlaceState,
    land: &LandCounts,
    date: Date,
    prices: &[u64],
    ledger: &mut super::PlaceProductionLedger,
) -> Result<(), DataError> {
    let jobs = BTreeMap::new();
    for building_index in 0..place.buildings.len() {
        let building = &place.buildings[building_index];
        let def = definition(rules, building)?;
        if def.methods.is_empty() {
            continue;
        }
        let free = free_capacity(def, building)?;
        let can_open = def
            .methods
            .iter()
            .any(|method| method.size_units > 0 && u64::from(method.size_units) <= free);
        if !can_open && (def.methods.len() < 2 || building.level_records.is_empty()) {
            continue;
        }
        let result = look_in_year(
            rules,
            place,
            land,
            building_index,
            date.month,
            Some(date.year),
            prices,
            &jobs,
        )?;
        if let Some(answer) = result.answer {
            make_move(
                rules,
                place,
                building_index,
                &result.moves[answer],
                date.year,
                date.month,
                ledger,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod fraction_tests {
    use super::*;
    use ck_sim::Canonical;
    fn fixture(annual: u64, good: &str) -> (Rules, PlaceState) {
        let mut rules = crate::tests::level_rules();
        rules.fertility_reference_pm = 1000;
        let mut other = rules
            .buildings
            .iter()
            .find(|b| b.id == "farm")
            .unwrap()
            .clone();
        other.id = "other".into();
        other.herds.clear();
        other.methods[0].annual_output = BTreeMap::from([(good.into(), annual)]);
        rules.buildings.push(other);
        let mut state = crate::tests::shipped_production_fixture(&rules);
        crate::tests::farm_with(
            &rules,
            &mut state,
            vec![crate::tests::level(0, 0, Some("pasture"), 13)],
        );
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        let mut producer = place.buildings[0].clone();
        producer.kind = "other".into();
        place.buildings.push(producer);
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        (rules, state.places.remove(0))
    }

    fn capacity_review_fixture() -> (Rules, PlaceState, LandCounts, Vec<u64>) {
        let (mut rules, mut place) = fixture(100, "oats");
        for good in &mut rules.goods {
            good.hunger_centi = 0;
            good.niceties_centi = 0;
            good.luxuries_centi = 0;
        }
        for pop_type in &mut rules.pop_types {
            pop_type.fuel_need_milli_day = 0;
        }
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods.truncate(2);
        let job = crate::ProductionJob {
            id: "maker".into(),
            places: 5,
            pop_types: vec!["peasant".into()],
            wage_weight_pm: 1000,
            manager: false,
        };
        def.methods[0].jobs = vec![job];
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 0)]);
        def.methods[0].made_by = BTreeMap::from([("oats".into(), "maker".into())]);
        def.methods[0].grows = false;
        def.methods[1] = def.methods[0].clone();
        def.methods[1].id = "processor".into();
        def.methods[1].resource = "none".into();
        def.methods[1].size_units = 1;
        def.methods[1].jobs[0].places = 1;
        def.methods[1].annual_output.insert("oats".into(), 10);
        place.buildings.truncate(1);
        for pop in &mut place.pops {
            pop.kind = "peasant".into();
            pop.workers = 0;
            pop.employment = vec![crate::Employment::default()];
        }
        place.pops[0].workers = 5;
        let entry = crate::Employment {
            workers: 5,
            jobs: BTreeMap::from([("maker".into(), 5)]),
        };
        place.pops[0].employment[0] = entry.clone();
        let building = &mut place.buildings[0];
        building.levels = 1;
        building.units = 5;
        building.method_levels = vec![1, 0];
        building.level_records.truncate(1);
        building.level_records[0].unpaid_work = BTreeMap::from([("oats".into(), Big::default())]);
        building.employment = vec![crate::Employment::default(); place.pops.len()];
        building.employment[0] = entry;
        building.production_carries.truncate(2);
        building.method_suitability.truncate(2);
        building.herds.fill(0);
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 20)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 20_000)]),
            fertile_cells: 20,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 20)]),
            sea: 0,
        };
        let prices = vec![1; rules.goods.len()];
        (rules, place, land, prices)
    }

    #[test]
    fn capacity_split_charges_one_combined_ramp_under_the_demand_cap() {
        let (mut rules, place, land, prices) = capacity_review_fixture();
        rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap()
            .methods[1]
            .grows = true;
        let look = look_in_year(
            &rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 15)]),
        )
        .unwrap();
        let entry = look.moves.iter().find(|entry| entry.method == 1).unwrap();
        assert_eq!(entry.plan.as_ref().unwrap().grounds.len(), 5);
        assert_eq!(entry.switching, Fraction::integer(15));
        assert_eq!(entry.slow_start, Fraction::ratio(5, 2));
        assert_eq!(entry.surplus.magnitude, Fraction::ratio(25, 2));
    }

    #[test]
    fn capacity_equal_replacements_charge_each_sources_unpaid_work() {
        let (mut rules, mut place, land, prices) = capacity_review_fixture();
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods[1].size_units = 5;
        def.methods[1].jobs[0].places = 5;
        def.methods[1].annual_output.insert("oats".into(), 100);
        let farm = &mut place.buildings[0];
        let mut second = farm.level_records[0].clone();
        second.number = 1;
        second
            .unpaid_work
            .insert("oats".into(), Big::new(5 * 144_000));
        farm.level_records.push(second);
        farm.method_levels[0] = 2;
        farm.levels = 2;
        farm.units = 11; // Free capacity forces look_capacity for the equal-size alternatives.
        let look = look_in_year(
            &rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 1_000)]),
        )
        .unwrap();
        let first = look
            .moves
            .iter()
            .find(|entry| {
                entry.method == 1 && entry.plan.as_ref().is_some_and(|p| p.sources == [0])
            })
            .unwrap();
        let second = look
            .moves
            .iter()
            .find(|entry| {
                entry.method == 1 && entry.plan.as_ref().is_some_and(|p| p.sources == [1])
            })
            .unwrap();
        assert_eq!(first.unpaid, Fraction::integer(0));
        assert_eq!(second.unpaid, Fraction::integer(5));
        assert_eq!(
            first.surplus.magnitude,
            second.surplus.magnitude.add(&Fraction::integer(5))
        );
        assert_eq!(
            look.moves[look.answer.unwrap()]
                .plan
                .as_ref()
                .unwrap()
                .sources,
            vec![0]
        );
    }

    #[test]
    fn aggregate_capacity_look_values_only_the_herds_marginal_difference() {
        let (mut rules, mut place, land, prices) = capacity_review_fixture();
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods[1].resource = "usable_land".into();
        def.herds.retain(|animal, _| animal == "cattle");
        let jobs = BTreeMap::from([("oats".into(), 1_000), ("dairy".into(), 1_000)]);
        let mut no_herd_rules = rules.clone();
        no_herd_rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap()
            .herds
            .clear();
        let without = look_in_year(
            &no_herd_rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &jobs,
        )
        .unwrap();
        place.buildings[0].herds[0] = 1;
        let with = look_in_year(&rules, &place, &land, 0, 5, Some(768), &prices, &jobs).unwrap();
        let a = without
            .moves
            .iter()
            .find(|entry| entry.method == 1)
            .unwrap();
        let b = with.moves.iter().find(|entry| entry.method == 1).unwrap();
        assert_eq!(a.plan.as_ref().unwrap().grounds.len(), 5);
        assert_eq!(a.staying, b.staying);
        assert_eq!(a.switching, b.switching);
        assert_eq!(a.surplus, b.surplus);
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods[1].resource = "none".into();
        let shrinking =
            look_in_year(&rules, &place, &land, 0, 5, Some(768), &prices, &jobs).unwrap();
        let c = shrinking
            .moves
            .iter()
            .find(|entry| entry.method == 1)
            .unwrap();
        assert!(
            c.staying.cmp(&a.staying) == Ordering::Greater,
            "lost herd output is charged"
        );
        assert_eq!(
            c.switching, a.switching,
            "the old herd is not added to the new method"
        );
    }

    #[test]
    fn splitting_to_a_non_land_method_does_not_found_herds_on_remaining_land() {
        let (mut rules, mut place, land, prices) = capacity_review_fixture();
        rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap()
            .methods[1]
            .annual_output
            .insert("oats".into(), 10_000);
        let farm = &mut place.buildings[0];
        let mut second = farm.level_records[0].clone();
        second.number = 1;
        farm.level_records.push(second);
        farm.method_levels[0] = 2;
        farm.levels = 2;
        farm.units = 10;
        let looked = look_in_year(
            &rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 100_000)]),
        )
        .unwrap();
        let chosen = &looked.moves[looked.answer.unwrap()];
        assert_eq!(chosen.plan.as_ref().unwrap().grounds.len(), 5);
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, chosen, 768, 5, &mut ledger).unwrap();
        assert_eq!(place.buildings[0].method_levels, vec![1, 5]);
        assert!(place.buildings[0].herds.iter().all(|heads| *heads == 0));
        assert!(ledger.herd_arrivals.iter().all(|heads| *heads == 0));
    }

    #[test]
    fn switch_rescales_job_carries_both_directions_across_save() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules
            .buildings
            .iter()
            .find(|kind| kind.id == "holdings")
            .unwrap();
        let oats = crate::tests::good_at(&rules, "oats");
        let building = &mut place.buildings[0];
        building.kind = "holdings".into();
        building.levels = 2;
        building.method_levels = vec![1, 1];
        building.production_carries = vec![vec![0; rules.goods.len()]; 2];
        building.production_carries[0][oats] = 25_000_000_000;
        rescale_carries(&rules, building, def, &[2, 0]).unwrap();
        building.method_levels = vec![2, 0];
        assert_eq!(building.production_carries[0][oats], 50_000_000_000);
        let state = crate::EconomyState {
            next_pop_serial: 1,
            places: vec![place],
            trade: crate::TradeCounters::default(),
        };
        let bytes = state.to_bytes().unwrap();
        let mut state = crate::EconomyState::from_bytes(&bytes).unwrap();
        let building = &mut state.places[0].buildings[0];
        super::super::check_production_carries(
            &rules,
            def,
            building,
            rules.fertility_reference_pm,
            "fixture",
        )
        .unwrap();
        rescale_carries(&rules, building, def, &[1, 1]).unwrap();
        building.method_levels = vec![1, 1];
        assert_eq!(building.production_carries[0][oats], 25_000_000_000);
        let bytes = state.to_bytes().unwrap();
        let state = crate::EconomyState::from_bytes(&bytes).unwrap();
        super::super::check_production_carries(
            &rules,
            def,
            &state.places[0].buildings[0],
            rules.fertility_reference_pm,
            "fixture",
        )
        .unwrap();
    }

    #[test]
    fn switch_hard_cap_kills_overflow_without_slaughter_yield() {
        let (rules, mut place) = fixture(100, "oats");
        let mut def = rules
            .buildings
            .iter()
            .find(|kind| kind.id == "demesne")
            .unwrap()
            .clone();
        def.methods.push(
            rules
                .buildings
                .iter()
                .find(|kind| kind.id == "holdings")
                .unwrap()
                .methods[1]
                .clone(),
        );
        let building = &mut place.buildings[0];
        building.method_levels = vec![2, 0];
        building.levels = 2;
        building.level_records = vec![
            crate::tests::level(0, 0, Some("pasture"), 13),
            crate::tests::level(1, 0, Some("pasture"), 13),
        ];
        building.herds = vec![30, 0, 0, 0];
        let (heads, slaughter, _, deaths) = switching_heads(
            &rules,
            building,
            &def,
            &building.level_records[0],
            &def.methods[1],
        )
        .unwrap();
        assert_eq!((heads[0], slaughter[0], deaths[0]), (24, 0, 6));
    }

    #[test]
    fn look_forecasts_changed_job_capacity_and_new_jobs() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules
            .buildings
            .iter()
            .find(|kind| kind.id == "holdings")
            .unwrap();
        let building = &mut place.buildings[0];
        building.method_levels = vec![1, 1];
        building.employment[0].jobs = BTreeMap::from([
            ("prior".into(), 2),
            ("monks".into(), 5),
            ("field_hands".into(), 5),
        ]);
        assert_eq!(
            forecast_staffing(def, building, &def.methods[0], "oats", &[2, 0]).unwrap(),
            (5, 10)
        );
        assert_eq!(
            forecast_staffing(def, building, &def.methods[0], "honey", &[2, 0]).unwrap(),
            (5, 10)
        );
    }

    #[test]
    fn forecast_adds_hiring_only_after_exact_transfers() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules.buildings.iter().find(|b| b.id == "holdings").unwrap();
        place.pops[0].kind = "labourer".into();
        place.pops[1].kind = "monk".into();
        let count = place.buildings.len();
        for pop in &mut place.pops {
            pop.employment.resize(count, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.kind = "holdings".into();
        building.levels = 1;
        building.method_levels = vec![0, 1];
        building.level_records = vec![crate::tests::level(0, 1, None, 13)];
        building.production_carries = vec![vec![0; rules.goods.len()]; 2];
        building.herds.fill(0);
        for entry in &mut building.employment {
            entry.jobs.clear();
            entry.workers = 0;
        }
        building.employment[0].jobs.insert("fishermen".into(), 2);
        building.employment[0].workers = 2;
        building.employment[1].jobs.insert("monks".into(), 4);
        building.employment[1].workers = 4;
        let (pools, forecast, factor) =
            projected_staffing(&place, &place.buildings[0], def, &[1, 0]).unwrap();
        let field = pools
            .iter()
            .position(|p| p.job.id == "field_hands")
            .unwrap();
        assert_eq!(forecast[field], Fraction::ratio(19, 5));
        assert_eq!(factor, Fraction::integer(500));
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 10,
        };
        let looked = look_in_year(
            &rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &vec![1000; rules.goods.len()],
            &BTreeMap::from([("oats".into(), 1_000_000)]),
        )
        .unwrap();
        assert_eq!(looked.moves.len(), 1);
        // 3.8 forecast field hands at 30,000 value each, including the candidate manager factor.
        assert_eq!(looked.moves[0].switching, Fraction::integer(114_000));
        let mut exact = place.buildings[0].clone();
        exact.method_levels = vec![1, 0];
        trim_jobs(&mut place.pops, &mut exact, def, 0).unwrap();
        assert_eq!(exact.employment[0].jobs.get("field_hands"), Some(&2));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One fixture follows split, capped demand, merge and opening through settlement."
    )]
    fn a_five_unit_level_splits_into_five_resource_free_levels() {
        let (mut rules, mut place) = fixture(100, "oats");
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.herds.clear();
        def.methods.truncate(2);
        let job = crate::ProductionJob {
            id: "maker".into(),
            places: 1,
            pop_types: vec!["peasant".into()],
            wage_weight_pm: 1000,
            manager: false,
        };
        def.methods[0].jobs = vec![job.clone()];
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 1)]);
        def.methods[0].made_by = BTreeMap::from([("oats".into(), "maker".into())]);
        def.methods[1] = def.methods[0].clone();
        def.methods[1].id = "processor".into();
        def.methods[1].resource = "none".into();
        def.methods[1].size_units = 1;
        def.methods[1].annual_output.insert("oats".into(), 10_000);
        def.methods[1].grows = false;
        for pop in &mut place.pops {
            pop.kind = "peasant".into();
            pop.workers = 0;
            pop.employment = vec![crate::Employment::default(); 2];
        }
        place.pops[0].workers = 1;
        let mut entry = crate::Employment {
            workers: 1,
            ..crate::Employment::default()
        };
        entry.jobs.insert("maker".into(), 1);
        place.pops[0].employment[0] = entry.clone();
        let building = &mut place.buildings[0];
        building.employment = vec![crate::Employment::default(); place.pops.len()];
        building.employment[0] = entry;
        building.method_levels = vec![1, 0];
        building.production_carries.truncate(2);
        building.method_suitability.truncate(2);
        building.level_records[0].unpaid_work = seasonal_accounts(
            &rules,
            rules.buildings.iter().find(|b| b.id == "farm").unwrap(),
            &rules
                .buildings
                .iter()
                .find(|b| b.id == "farm")
                .unwrap()
                .methods[0],
        );
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 0,
        };
        let prices = vec![1000; rules.goods.len()];
        // Demand is capped once for all created levels: 50 oats against 15 wanted loses to 20 fish against 20 wanted.
        let mut demand_rules = rules.clone();
        for good in &mut demand_rules.goods {
            good.hunger_centi = 0;
            good.niceties_centi = 0;
            good.luxuries_centi = 0;
        }
        for kind in &mut demand_rules.pop_types {
            kind.fuel_need_milli_day = 0;
        }
        let def = demand_rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods[0].jobs[0].places = 5;
        def.methods[0].annual_output.insert("oats".into(), 0);
        def.methods[1].annual_output.insert("oats".into(), 10);
        let mut third = def.methods[1].clone();
        third.id = "fish_processor".into();
        third.size_units = 5;
        third.jobs[0].places = 5;
        third.annual_output = BTreeMap::from([("fish".into(), 20)]);
        third.made_by = BTreeMap::from([("fish".into(), "maker".into())]);
        def.methods.push(third);
        let mut demand_place = place.clone();
        demand_place.buildings.truncate(1);
        for pop in &mut demand_place.pops {
            pop.employment.truncate(1);
        }
        demand_place.pops[0].workers = 5;
        demand_place.pops[0].employment[0].workers = 5;
        demand_place.pops[0].employment[0]
            .jobs
            .insert("maker".into(), 5);
        let farm = &mut demand_place.buildings[0];
        farm.employment[0] = demand_place.pops[0].employment[0].clone();
        farm.method_levels = vec![1, 0, 0];
        farm.production_carries
            .push(vec![0; demand_rules.goods.len()]);
        farm.method_suitability
            .push(vec![0; demand_rules.goods.len()]);
        farm.level_records[0].unpaid_work.clear();
        let one_prices = vec![1; demand_rules.goods.len()];
        let demand = look_in_year(
            &demand_rules,
            &demand_place,
            &land,
            0,
            5,
            Some(768),
            &one_prices,
            &BTreeMap::from([("oats".into(), 15), ("fish".into(), 20)]),
        )
        .unwrap();
        let b = demand.moves.iter().find(|item| item.method == 1).unwrap();
        let c = demand.moves.iter().find(|item| item.method == 2).unwrap();
        assert_eq!(b.switching, Fraction::integer(15));
        assert_eq!(c.switching, Fraction::integer(20));
        assert_eq!(demand.moves[demand.answer.unwrap()].method, 2);
        demand_place.buildings[0].units = 6;
        let tied = look_in_year(
            &demand_rules,
            &demand_place,
            &land,
            0,
            5,
            Some(768),
            &one_prices,
            &BTreeMap::from([("oats".into(), 5), ("fish".into(), 5)]),
        )
        .unwrap();
        let best = &tied.moves[tied.answer.unwrap()];
        assert_eq!(best.method, 1);
        assert!(
            best.plan.as_ref().unwrap().sources.is_empty(),
            "an opening wins the equal-surplus tie"
        );

        let mut leftover_rules = rules.clone();
        leftover_rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap()
            .methods[1]
            .size_units = 2;
        let mut leftover_place = place.clone();
        let leftover = look_in_year(
            &leftover_rules,
            &leftover_place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 1_000_000)]),
        )
        .unwrap();
        let chosen = &leftover.moves[leftover.answer.unwrap()];
        assert_eq!(chosen.plan.as_ref().unwrap().grounds.len(), 2);
        let mut ledger = super::super::PlaceProductionLedger::new(
            &leftover_place.id,
            leftover_rules.goods.len(),
            leftover_rules.herd_species.len(),
        );
        make_move(
            &leftover_rules,
            &mut leftover_place,
            0,
            chosen,
            768,
            5,
            &mut ledger,
        )
        .unwrap();
        assert_eq!(
            free_capacity(
                definition(&leftover_rules, &leftover_place.buildings[0]).unwrap(),
                &leftover_place.buildings[0]
            )
            .unwrap(),
            1
        );

        let looked = look_in_year(
            &rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 1_000_000)]),
        )
        .unwrap();
        let chosen = &looked.moves[looked.answer.unwrap()];
        assert_eq!(chosen.method, 1);
        let plan = chosen.plan.as_ref().unwrap();
        assert_eq!(plan.sources, vec![0]);
        assert_eq!(plan.grounds, vec![None; 5]);
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, chosen, 768, 5, &mut ledger).unwrap();
        let building = &place.buildings[0];
        assert_eq!(building.units, 5);
        assert_eq!(building.method_levels, vec![0, 5]);
        assert_eq!(
            building
                .level_records
                .iter()
                .map(|record| record.number)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(
            building
                .level_records
                .iter()
                .all(|record| record.ground.is_none()
                    && record.last_switch == Some((768, 5))
                    && record.paydays == 13)
        );
        assert_eq!(building.employment[0].workers, 1);

        // The five small levels can merge into one field level in the following month.
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods[0].annual_output.insert("oats".into(), 50_000);
        def.methods[1].annual_output.insert("oats".into(), 1);
        let mut weighted = place.clone();
        weighted.buildings[0].units = 7;
        for (index, record) in weighted.buildings[0].level_records.iter_mut().enumerate() {
            record.unpaid_work.insert(
                "oats".into(),
                Big::new(if index == 0 { 1_000_000 } else { index as u128 }),
            );
        }
        let ranked = look_in_year(
            &rules,
            &weighted,
            &land,
            0,
            6,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 1_000_000)]),
        )
        .unwrap();
        let chosen = &ranked.moves[ranked.answer.unwrap()];
        assert_eq!(chosen.plan.as_ref().unwrap().sources, vec![1, 2, 3]);
        let mut ledger = super::super::PlaceProductionLedger::new(
            &weighted.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut weighted, 0, chosen, 768, 6, &mut ledger).unwrap();
        assert_eq!(weighted.buildings[0].method_levels, vec![1, 2]);
        assert_eq!(weighted.buildings[0].employment[0].workers, 1);

        let looked = look_in_year(
            &rules,
            &place,
            &land,
            0,
            6,
            Some(768),
            &prices,
            &BTreeMap::from([("oats".into(), 1_000_000)]),
        )
        .unwrap();
        let chosen = &looked.moves[looked.answer.unwrap()];
        assert_eq!(chosen.method, 0);
        assert_eq!(chosen.plan.as_ref().unwrap().sources, vec![0, 1, 2, 3, 4]);
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, chosen, 768, 6, &mut ledger).unwrap();
        assert_eq!(place.buildings[0].method_levels, vec![1, 0]);
        assert_eq!(place.buildings[0].units, 5);
        assert_eq!(
            place.buildings[0].level_records[0].ground.as_deref(),
            Some("pasture")
        );

        // An empty, one-method building opens through the monthly entry point without land or sea.
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        let mut processor = def.methods[1].clone();
        processor.size_units = 2;
        processor.jobs[0].places = 2;
        processor.annual_output.insert("oats".into(), 12_000);
        def.methods = vec![processor];
        place.buildings.truncate(1);
        for pop in &mut place.pops {
            pop.employment = vec![crate::Employment::default()];
            pop.stock.fill(0);
        }
        place.pops[0].workers = 2;
        let building = &mut place.buildings[0];
        building.levels = 0;
        building.units = 2;
        building.method_levels = vec![0];
        building.level_records.clear();
        building.employment = vec![crate::Employment::default(); place.pops.len()];
        building.production_carries.truncate(1);
        building.method_suitability = vec![vec![0; rules.goods.len()]];
        let empty_land = LandCounts::default();
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        run_place_switches(
            &rules,
            &mut place,
            &empty_land,
            Date {
                year: 768,
                month: 6,
                day: 1,
                hour: 12,
            },
            &prices,
            &mut ledger,
        )
        .unwrap();
        assert_eq!(place.buildings[0].method_levels, vec![1]);
        assert_eq!(place.buildings[0].level_records[0].ground, None);
        assert_eq!(place.buildings[0].employment[0].workers, 0);
        super::super::fill_monthly_hiring(&rules, &mut place).unwrap();
        assert_eq!(place.buildings[0].employment[0].workers, 1);
        super::super::fill_monthly_hiring(&rules, &mut place).unwrap();
        assert_eq!(place.buildings[0].employment[0].workers, 2);
        rules.buildings.retain(|building| building.id != "other");
        let mut state = super::super::EconomyState {
            places: vec![place],
            trade: super::super::TradeCounters::default(),
            next_pop_serial: 1,
        };
        super::super::run_monthly_production(
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
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        assert!(state.places[0].buildings[0].last_made[oats] > 0);
        super::super::validate_level_records(&rules, &BTreeMap::new(), &state).unwrap();
        let mut bytes = Vec::new();
        state.encode(&mut bytes).unwrap();
        let mut reader = ck_sim::Reader::new(&bytes);
        let restored = super::super::EconomyState::decode(&mut reader).unwrap();
        assert_eq!(restored, state);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The two-pop staffing and herd witness compares the look with settlement."
    )]
    fn multi_source_settlement_matches_the_look_and_releases_net_excess() {
        let (mut rules, mut place) = fixture(100, "oats");
        let def = rules
            .buildings
            .iter_mut()
            .find(|item| item.id == "farm")
            .unwrap();
        def.methods.truncate(2);
        let job = crate::ProductionJob {
            id: "maker".into(),
            places: 2,
            pop_types: vec!["peasant".into()],
            wage_weight_pm: 1000,
            manager: false,
        };
        def.methods[0].jobs = vec![job.clone()];
        def.methods[0].size_units = 1;
        def.methods[1] = def.methods[0].clone();
        def.methods[1].id = "processor".into();
        def.methods[1].resource = "none".into();
        def.methods[1].size_units = 2;
        def.methods[1].jobs[0].places = 1;
        for pop in &mut place.pops {
            pop.kind = "peasant".into();
            pop.employment = vec![crate::Employment::default(); 2];
        }
        for (index, workers) in [(0, 2), (1, 1)] {
            place.pops[index].workers = workers;
            place.pops[index].employment[0] = crate::Employment {
                workers,
                jobs: BTreeMap::from([("maker".into(), workers)]),
            };
        }
        let farm = &mut place.buildings[0];
        farm.levels = 2;
        farm.units = 2;
        farm.method_levels = vec![2, 0];
        farm.level_records = vec![
            crate::tests::level(0, 0, Some("pasture"), 13),
            crate::tests::level(1, 0, Some("pasture"), 13),
        ];
        farm.employment = place
            .pops
            .iter()
            .map(|pop| pop.employment[0].clone())
            .collect();
        farm.production_carries.truncate(2);
        farm.method_suitability.truncate(2);
        farm.herds = vec![0; rules.herd_species.len()];
        farm.herds[0] = 8;
        let def = rules
            .buildings
            .iter()
            .find(|item| item.id == "farm")
            .unwrap();
        let mut exact = farm.clone();
        let mut pops = place.pops.clone();
        replace_levels(
            &rules,
            &mut pops,
            &mut exact,
            def,
            0,
            &[0, 1],
            1,
            &[None],
            768,
            5,
        )
        .unwrap();
        let expected_herds = switching_heads_for(&rules, &exact, def, &exact.method_levels, false)
            .unwrap()
            .0;
        assert_eq!(
            exact
                .employment
                .iter()
                .map(|entry| entry.workers)
                .collect::<Vec<_>>(),
            vec![1, 0, 0]
        );
        let zero = Fraction::integer(0);
        let entry = MoveLook {
            level: 0,
            method: 1,
            ground: None,
            plan: Some(MovePlan {
                sources: vec![0, 1],
                grounds: vec![None],
            }),
            staying: zero.clone(),
            switching: zero.clone(),
            unpaid: zero.clone(),
            slow_start: zero.clone(),
            herd_cost: zero.clone(),
            surplus: SignedFraction::difference(&zero, &zero),
        };
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, &entry, 768, 5, &mut ledger).unwrap();
        assert_eq!(place.buildings[0].employment, exact.employment);
        assert_eq!(place.buildings[0].herds, expected_herds);
        assert_eq!(place.buildings[0].herds[0], 0);
    }

    #[test]
    fn capacity_ground_selection_releases_sources_then_reserves_each_target() {
        let (rules, _) = fixture(100, "oats");
        let def = rules
            .buildings
            .iter()
            .find(|item| item.id == "farm")
            .unwrap();
        let one = crate::tests::level(0, 0, Some("pasture"), 13);
        let two = crate::tests::level(1, 0, Some("pasture"), 13);
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 4)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 4_000)]),
            fertile_cells: 4,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 4)]),
            sea: 0,
        };
        let usage = Usage {
            by_ground: BTreeMap::from([("pasture".into(), 4_000)]),
            cells: BTreeMap::new(),
        };
        let grounds = capacity_grounds(
            &rules,
            def,
            &land,
            &usage,
            &[&one, &two],
            &def.methods[0],
            3,
        );
        assert_eq!(
            grounds,
            vec![Some("pasture".into()), Some("pasture".into())]
        );
    }

    #[test]
    fn empty_building_forecast_shares_unemployed_workers_between_jobs() {
        let (rules, mut place) = fixture(100, "oats");
        let mut def = rules
            .buildings
            .iter()
            .find(|item| item.id == "farm")
            .unwrap()
            .clone();
        let job = |id: &str, manager| crate::ProductionJob {
            id: id.into(),
            places: 1,
            pop_types: vec!["peasant".into()],
            wage_weight_pm: 1000,
            manager,
        };
        def.methods[0].jobs = vec![job("maker", false), job("steward", true)];
        place.buildings.truncate(1);
        for pop in &mut place.pops {
            pop.kind = "peasant".into();
            pop.workers = 0;
            pop.employment = vec![crate::Employment::default()];
        }
        place.pops[0].workers = 1;
        let building = &mut place.buildings[0];
        building.levels = 0;
        building.level_records.clear();
        building.method_levels.fill(0);
        building.employment = vec![crate::Employment::default(); place.pops.len()];
        let mut levels = vec![0; def.methods.len()];
        levels[0] = 1;
        let (pools, forecast, _) =
            projected_staffing(&place, &place.buildings[0], &def, &levels).unwrap();
        assert_eq!(pools[0].job.id, "steward");
        assert_eq!(forecast, vec![Fraction::integer(1), Fraction::integer(0)]);
        place.pops[0].workers = 2;
        let (_, forecast, _) =
            projected_staffing(&place, &place.buildings[0], &def, &levels).unwrap();
        assert_eq!(forecast, vec![Fraction::integer(1), Fraction::integer(1)]);
        assert!(
            place.buildings[0]
                .employment
                .iter()
                .all(|entry| entry.workers == 0)
        );
    }

    #[test]
    fn replacing_multiple_levels_rebuilds_one_final_pool_and_renumbers() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules
            .buildings
            .iter()
            .find(|item| item.id == "farm")
            .unwrap();
        let mut record = place.buildings[0].level_records[0].clone();
        record.number = 1;
        place.buildings[0].level_records.push(record);
        place.buildings[0].levels = 2;
        place.buildings[0].units = 10;
        place.buildings[0].method_levels[0] = 2;
        replace_levels(
            &rules,
            &mut place.pops,
            &mut place.buildings[0],
            def,
            0,
            &[0, 1],
            1,
            &[None],
            768,
            5,
        )
        .unwrap();
        let building = &place.buildings[0];
        assert_eq!(building.levels, 1);
        assert_eq!(building.units, 10);
        assert_eq!(building.method_levels, vec![0, 1, 0, 0]);
        assert_eq!(building.level_records[0].number, 0);
        assert_eq!(building.level_records[0].last_switch, Some((768, 5)));
        assert_eq!(building.level_records[0].paydays, 13);
    }

    #[test]
    fn forecast_manager_penalty_uses_the_same_floor_as_payday() {
        let (rules, mut place) = fixture(100, "oats");
        let mut def = rules
            .buildings
            .iter()
            .find(|b| b.id == "holdings")
            .unwrap()
            .clone();
        let mut ordinary = def.methods[0]
            .jobs
            .iter()
            .find(|j| j.id == "field_hands")
            .unwrap()
            .clone();
        ordinary.places = 3;
        let manager = def.methods[0]
            .jobs
            .iter()
            .find(|j| j.id == "prior")
            .unwrap()
            .clone();
        def.methods[0].jobs = vec![ordinary.clone()];
        def.methods[1].jobs = vec![ordinary, manager];
        place.pops[0].kind = "labourer".into();
        for pop in &mut place.pops {
            pop.employment.resize(2, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.levels = 1;
        building.method_levels = vec![1, 0];
        for entry in &mut building.employment {
            entry.jobs.clear();
            entry.workers = 0;
        }
        building.employment[0].jobs.insert("field_hands".into(), 1);
        building.employment[0].workers = 1;
        let (_, _, factor) =
            projected_staffing(&place, &place.buildings[0], &def, &[0, 1]).unwrap();
        assert_eq!(factor, Fraction::integer(667));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The look and settlement share one staffing fixture."
    )]
    fn manager_first_transfer_allows_herd_founders() {
        let (mut rules, mut place) = fixture(100, "oats");
        let idx = rules
            .buildings
            .iter()
            .position(|b| b.id == "demesne")
            .unwrap();
        let mut target = rules.buildings[idx].methods[0].clone();
        target.id = "target".into();
        target
            .jobs
            .retain(|j| j.id == "steward" || j.id == "herder");
        target.annual_output = BTreeMap::from([("oats".into(), 100)]);
        target.made_by = BTreeMap::from([("oats".into(), "herder".into())]);
        let mut source = target.clone();
        source.id = "source".into();
        source.jobs.retain(|j| j.id == "steward");
        let mut old = source.jobs[0].clone();
        old.id = "old".into();
        old.manager = false;
        old.places = 2;
        source.jobs.push(old);
        rules.buildings[idx].methods = vec![source, target];
        let def = &rules.buildings[idx];
        place.pops[0].kind = "labourer".into();
        let count = place.buildings.len();
        for pop in &mut place.pops {
            pop.employment.resize(count, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.kind = "demesne".into();
        building.levels = 1;
        building.method_levels = vec![1, 0];
        building.production_carries = vec![vec![0; rules.goods.len()]; 2];
        building.level_records = vec![crate::tests::level(0, 0, Some("pasture"), 13)];
        building.herds.fill(0);
        for entry in &mut building.employment {
            entry.jobs.clear();
            entry.workers = 0;
        }
        building.employment[0].jobs.insert("old".into(), 2);
        building.employment[0].workers = 2;
        let original = building.clone();
        let mut exact = original.clone();
        exact.method_levels = vec![0, 1];
        trim_jobs(&mut place.pops, &mut exact, def, 0).unwrap();
        assert_eq!(exact.employment[0].jobs.get("steward"), Some(&1));
        assert_eq!(exact.employment[0].jobs.get("herder"), Some(&1));
        let mut basis = original.clone();
        basis.employment = exact.employment.clone();
        let (heads, gone, founders, dead) = switching_heads(
            &rules,
            &basis,
            def,
            &basis.level_records[0],
            &def.methods[1],
        )
        .unwrap();
        assert_eq!((heads[0], gone[0], founders[0], dead[0]), (2, 0, 2, 0));
        let (_, _, factor) = projected_staffing(&place, &original, def, &[0, 1]).unwrap();
        assert_eq!(factor, Fraction::integer(1000));
        let dairy = crate::tests::good_at(&rules, "dairy");
        let present_value = herd_output(&rules, &place, &original, def, &heads, None).unwrap();
        let candidate_value =
            herd_output(&rules, &place, &original, def, &heads, Some(&factor)).unwrap();
        assert_ne!(present_value[dairy], Fraction::integer(0));
        assert_eq!(
            candidate_value[dairy],
            present_value[dairy].mul(&Fraction::integer(2))
        );
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 0,
        };
        let prices = vec![1000; rules.goods.len()];
        let looked = look_in_year(
            &rules,
            &place,
            &land,
            0,
            5,
            Some(768),
            &prices,
            &BTreeMap::from([("dairy".into(), 1_000_000), ("oats".into(), 1_000_000)]),
        )
        .unwrap();
        assert_eq!(looked.moves.len(), 1);
        assert_eq!(looked.moves[0].herd_cost, Fraction::integer(8_000));
        assert_eq!(looked.moves[0].switching, Fraction::integer(244_000));
        let z = Fraction::integer(0);
        let entry = MoveLook {
            level: 0,
            method: 1,
            ground: Some("pasture".into()),
            plan: None,
            staying: z.clone(),
            switching: z.clone(),
            unpaid: z.clone(),
            slow_start: z.clone(),
            herd_cost: z.clone(),
            surplus: SignedFraction::difference(&z, &z),
        };
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, &entry, 768, 5, &mut ledger).unwrap();
        assert_eq!(place.buildings[0].employment, exact.employment);
        assert_eq!(place.buildings[0].herds[0], 2);
        assert_eq!(ledger.herd_arrivals[0], 2);
    }

    #[test]
    fn holdings_switches_transfer_staff_and_start_by_growth_flag() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules.buildings.iter().find(|b| b.id == "holdings").unwrap();
        assert!(def.methods[0].grows);
        assert!(!def.methods[1].grows);
        place.pops[0].kind = "labourer".into();
        let count = place.buildings.len();
        for pop in &mut place.pops {
            pop.employment.resize(count, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.kind = "holdings".into();
        building.levels = 1;
        building.method_levels = vec![0, 1];
        building.production_carries = vec![vec![0; rules.goods.len()]; 2];
        building.level_records = vec![crate::tests::level(0, 1, None, 13)];
        building.level_records[0].unpaid_work = seasonal_accounts(&rules, def, &def.methods[1]);
        building.level_records[0]
            .unpaid_work
            .get_mut("fish")
            .unwrap()
            .clone_from(&Big::new(42));
        building.herds.fill(0);
        for entry in &mut building.employment {
            entry.jobs.clear();
            entry.workers = 0;
        }
        building.employment[0].jobs.insert("fishermen".into(), 5);
        building.employment[0].workers = 5;
        let z = Fraction::integer(0);
        let mut entry = MoveLook {
            level: 0,
            method: 0,
            ground: Some("pasture".into()),
            plan: None,
            staying: z.clone(),
            switching: z.clone(),
            unpaid: z.clone(),
            slow_start: z.clone(),
            herd_cost: z.clone(),
            surplus: SignedFraction::difference(&z, &z),
        };
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, &entry, 768, 5, &mut ledger).unwrap();
        let building = &place.buildings[0];
        assert_eq!(building.employment[0].jobs.get("field_hands"), Some(&5));
        assert_eq!(building.employment[0].workers, 5);
        assert_eq!(building.level_records[0].paydays, 0);
        assert!(!building.level_records[0].unpaid_work.contains_key("fish"));
        entry.method = 1;
        entry.ground = None;
        make_move(&rules, &mut place, 0, &entry, 768, 6, &mut ledger).unwrap();
        let building = &place.buildings[0];
        assert_eq!(building.employment[0].jobs.get("fishermen"), Some(&5));
        assert_eq!(building.employment[0].workers, 5);
        assert_eq!(building.level_records[0].paydays, 13);
        assert!(!building.level_records[0].unpaid_work.contains_key("oats"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The switch, forecast cost and consecutive seasonal payments form one witness."
    )]
    fn fishing_after_switch_pays_full_first_season_shares() {
        let (mut rules, mut place) = fixture(100, "oats");
        let def = rules.buildings.iter().find(|b| b.id == "holdings").unwrap();
        place.buildings.truncate(1);
        place.pops[0].kind = "labourer".into();
        place.pops[1].kind = "monk".into();
        for pop in &mut place.pops {
            pop.employment.resize(1, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.kind = "holdings".into();
        building.levels = 1;
        building.method_levels = vec![1, 0];
        building.production_carries = vec![vec![0; rules.goods.len()]; 2];
        building.level_records = vec![crate::tests::level(0, 0, Some("pasture"), 13)];
        building.level_records[0].unpaid_work = seasonal_accounts(&rules, def, &def.methods[0]);
        building.herds.fill(0);
        for entry in &mut building.employment {
            entry.jobs.clear();
            entry.workers = 0;
        }
        building.employment[0].jobs.insert("field_hands".into(), 5);
        building.employment[0].workers = 5;
        building.employment[1].jobs = BTreeMap::from([("prior".into(), 1), ("monks".into(), 5)]);
        building.employment[1].workers = 6;
        building.level_records[0]
            .unpaid_work
            .get_mut("oats")
            .unwrap()
            .clone_from(&Big::new(3 * 144_000));
        place.pops[0].workers = 5;
        place.pops[1].workers = 6;
        for (pop, employment) in place.pops.iter_mut().zip(&building.employment) {
            pop.employment[0] = employment.clone();
        }
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 1,
        };
        let look = look_in_year(
            &rules,
            &place,
            &land,
            0,
            4,
            Some(768),
            &vec![1; rules.goods.len()],
            &BTreeMap::new(),
        )
        .unwrap();
        let candidate = look.moves.iter().find(|entry| entry.method == 1).unwrap();
        assert_eq!(candidate.unpaid, Fraction::integer(3));
        assert_eq!(candidate.slow_start, Fraction::integer(0));
        let z = Fraction::integer(0);
        let entry = MoveLook {
            level: 0,
            method: 1,
            ground: None,
            plan: None,
            staying: z.clone(),
            switching: z.clone(),
            unpaid: z.clone(),
            slow_start: z.clone(),
            herd_cost: z.clone(),
            surplus: SignedFraction::difference(&z, &z),
        };
        let mut ledger = super::super::PlaceProductionLedger::new(
            &place.id,
            rules.goods.len(),
            rules.herd_species.len(),
        );
        make_move(&rules, &mut place, 0, &entry, 768, 4, &mut ledger).unwrap();
        rules.buildings.retain(|b| b.id != "other");
        let mut state = crate::tests::shipped_production_fixture(&rules);
        state.places = vec![place];
        let good = |id: &str| crate::tests::good_at(&rules, id);
        let mut made = Vec::new();
        for month in 5..=7 {
            let ledger = super::super::run_monthly_production(
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
            made.push([
                ledger.produced[good("fish")],
                ledger.produced[good("seabirds")],
                ledger.produced[good("herbs")],
                ledger.produced[good("vegetables")],
            ]);
        }
        assert_eq!(
            made,
            vec![[1280, 501, 0, 0], [1280, 499, 2, 0], [1280, 500, 3, 5]]
        );
    }

    #[test]
    fn transfer_fills_existing_job_and_leaves_ineligible_workers_unemployed() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules.buildings.iter().find(|b| b.id == "holdings").unwrap();
        place.pops[0].kind = "labourer".into();
        place.pops[1].kind = "noble".into();
        let count = place.buildings.len();
        for pop in &mut place.pops {
            pop.employment.resize(count, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.method_levels = vec![2, 0];
        for entry in &mut building.employment {
            entry.jobs.clear();
            entry.workers = 0;
        }
        building.employment[0].jobs =
            BTreeMap::from([("field_hands".into(), 5), ("fishermen".into(), 5)]);
        building.employment[0].workers = 10;
        building.employment[1].jobs.insert("fishermen".into(), 1);
        building.employment[1].workers = 1;
        trim_jobs(&mut place.pops, building, def, 0).unwrap();
        assert_eq!(building.employment[0].jobs.get("field_hands"), Some(&10));
        assert_eq!(building.employment[1].workers, 0);
        assert_eq!(place.pops[1].employment[0].workers, 0);
    }

    #[test]
    fn released_fishermen_fill_new_field_hand_places() {
        let (rules, mut place) = fixture(100, "oats");
        let def = rules.buildings.iter().find(|b| b.id == "holdings").unwrap();
        place.pops[0].kind = "labourer".into();
        let count = place.buildings.len();
        for pop in &mut place.pops {
            pop.employment.resize(count, crate::Employment::default());
        }
        let building = &mut place.buildings[0];
        building.method_levels = vec![1, 0];
        building.employment[0].jobs.clear();
        building.employment[0].jobs.insert("fishermen".into(), 5);
        building.employment[0].workers = 5;
        trim_jobs(&mut place.pops, building, def, 0).unwrap();
        assert_eq!(building.employment[0].jobs.get("field_hands"), Some(&5));
        assert_eq!(building.employment[0].workers, 5);
    }

    #[test]
    fn a_switch_releases_excess_job_workers_in_pop_order() {
        let (rules, mut place) = fixture(100, "oats");
        let definition = rules
            .buildings
            .iter()
            .find(|kind| kind.id == "holdings")
            .unwrap();
        let count = place.buildings.len();
        let building = &mut place.buildings[0];
        building.method_levels = vec![1, 0];
        for index in 0..2 {
            building.employment[index]
                .jobs
                .insert("field_hands".into(), 4);
            building.employment[index].workers = 4;
            place.pops[index]
                .employment
                .resize(count, crate::Employment::default());
        }
        trim_jobs(&mut place.pops, &mut place.buildings[0], definition, 0).unwrap();
        assert_eq!(place.buildings[0].employment[0].jobs["field_hands"], 2);
        assert_eq!(place.buildings[0].employment[1].jobs["field_hands"], 3);
        assert_eq!(place.pops[1].employment[0].jobs["field_hands"], 3);
    }

    #[test]
    fn stocked_population_orders_leave_nine_hundred_after_other_levels() {
        let (mut rules, mut place) = fixture(3100, "oats");
        let oats = crate::tests::good_at(&rules, "oats");
        rules.goods[oats].hunger_centi = 36_500;
        rules
            .diet_profiles
            .insert("peasant".into(), BTreeMap::from([("oats".into(), 1000)]));
        place.pops[0].workers = 4000;
        place.pops[0].stock[oats] = 1_000_000;
        let want = wanted(&rules, &place, &BTreeMap::new(), 0, 0, 3, 1000).unwrap();
        assert_eq!(want.goods[oats], Fraction::integer(900));
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.herds.clear();
        farm.methods[0].annual_output = BTreeMap::from([("oats".into(), 900)]);
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 20)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 20_000)]),
            fertile_cells: 20,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 1,
        };
        let result = look(
            &rules,
            &place,
            &land,
            0,
            3,
            &vec![1; rules.goods.len()],
            &BTreeMap::new(),
        )
        .unwrap();
        let sea = result.moves.iter().find(|m| m.method == 1).unwrap();
        assert_eq!(sea.staying, Fraction::integer(900));
        let mut made = vec![Fraction::integer(0); rules.goods.len()];
        made[oats] = Fraction::integer(900);
        assert_eq!(
            value(&rules, &made, &want, &vec![1; rules.goods.len()]),
            Fraction::integer(900)
        );
        assert_eq!(
            super::super::daily_market_orders(&rules, &place)
                .unwrap()
                .buy_units[oats],
            0
        );
    }

    #[test]
    fn other_wood_fills_the_job_before_heat_and_unwanted_turf_is_worth_nothing() {
        let (mut rules, mut place) = fixture(1, "hardwood");
        place.pops[0].workers = 200;
        rules
            .pop_types
            .iter_mut()
            .find(|p| p.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 1000;
        let jobs = BTreeMap::from([("hardwood".into(), 1)]);
        let want = wanted(&rules, &place, &jobs, 0, 0, 3, 1000).unwrap();
        let wood = crate::tests::good_at(&rules, "hardwood");
        assert_eq!(want.jobs[wood], Fraction::integer(0));
        assert_eq!(want.heat, Fraction::integer(73_000));
        let peat = crate::tests::good_at(&rules, "peat");
        let mut offered = vec![Fraction::integer(0); rules.goods.len()];
        offered[peat] = Fraction::integer(1);
        assert_eq!(
            value(&rules, &offered, &want, &vec![1; rules.goods.len()]),
            Fraction::integer(1)
        );
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "other")
            .unwrap()
            .methods[0]
            .annual_output
            .insert("hardwood".into(), 73_000);
        let one_heat = wanted(&rules, &place, &jobs, 0, 0, 3, 1000).unwrap();
        assert_eq!(one_heat.heat, Fraction::integer(1));
        assert_eq!(
            value(&rules, &offered, &one_heat, &vec![1; rules.goods.len()]),
            Fraction::integer(1)
        );
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "other")
            .unwrap()
            .methods[0]
            .annual_output
            .insert("hardwood".into(), 67_001);
        let six_thousand = wanted(&rules, &place, &jobs, 0, 0, 3, 1000).unwrap();
        assert_eq!(six_thousand.heat, Fraction::integer(6000));
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "other")
            .unwrap()
            .methods[0]
            .annual_output
            .insert("hardwood".into(), 73_001);
        let want = wanted(&rules, &place, &jobs, 0, 0, 3, 1000).unwrap();
        assert_eq!(want.heat, Fraction::integer(0));
        assert_eq!(
            value(&rules, &offered, &want, &vec![999; rules.goods.len()]),
            Fraction::integer(0)
        );
        // With no heat demand and no other wood, the job still values its load.
        place.pops[0].workers = 0;
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "other")
            .unwrap()
            .methods[0]
            .annual_output
            .insert("hardwood".into(), 0);
        let only_job = wanted(&rules, &place, &jobs, 0, 0, 3, 1000).unwrap();
        assert_eq!(only_job.heat, Fraction::integer(0));
        assert_eq!(only_job.jobs[wood], Fraction::integer(1));
        let mut wood_output = vec![Fraction::integer(0); rules.goods.len()];
        wood_output[wood] = Fraction::integer(1);
        assert_eq!(
            value(&rules, &wood_output, &only_job, &vec![1; rules.goods.len()]),
            Fraction::integer(1)
        );
    }

    #[test]
    fn two_paydays_are_added_before_the_demand_cap() {
        let (mut rules, place) = fixture(16, "oats");
        rules
            .seasons
            .insert("grain".into(), vec![500, 500, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let building = &place.buildings[1];
        let def = definition(&rules, building).unwrap();
        let record = &building.level_records[0];
        let quantities = output(
            &rules,
            &place,
            building,
            def,
            &def.methods[0],
            record.ground.as_deref(),
            13,
            12,
            1000,
            true,
            None,
        )
        .unwrap();
        let oats = crate::tests::good_at(&rules, "oats");
        assert_eq!(quantities[oats], Fraction::integer(16));
        let mut want = Wanted {
            goods: vec![Fraction::integer(0); rules.goods.len()],
            jobs: vec![Fraction::integer(0); rules.goods.len()],
            heat: Fraction::integer(0),
        };
        want.goods[oats] = Fraction::integer(10);
        assert_eq!(
            value(&rules, &quantities, &want, &vec![1; rules.goods.len()]),
            Fraction::integer(10)
        );
    }

    #[test]
    fn herd_capacity_slaughter_and_founders_match_the_worked_cases() {
        let (mut rules, mut place) = fixture(1, "oats");
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.herds.retain(|name, _| name == "cattle");
        def.herds.get_mut("cattle").unwrap().maximum = 12;
        let cattle = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        rules.herd_species[cattle].yields = vec![super::super::HerdYield {
            good: "beef".into(),
            units: 150,
        }];
        let def = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let farm = &mut place.buildings[0];
        farm.levels = 10;
        farm.units = 50;
        farm.method_levels = vec![10, 0, 0, 0];
        farm.level_records = (0..10)
            .map(|n| crate::tests::level(n, 0, Some("pasture"), 13))
            .collect();
        let mut prices = vec![1; rules.goods.len()];
        prices[crate::tests::good_at(&rules, "cattle")] = 240;
        for (heads, expected) in [(100, 0), (118, 900)] {
            farm.herds[cattle] = heads;
            let (_, gone, founders, _) =
                switching_heads(&rules, farm, def, &farm.level_records[0], &def.methods[1])
                    .unwrap();
            assert_eq!(
                herd_loss(&rules, &gone, &founders, &prices).unwrap(),
                Fraction::integer(expected)
            );
        }
        prices[crate::tests::good_at(&rules, "cattle")] = 100;
        let (_, gone, founders, _) =
            switching_heads(&rules, farm, def, &farm.level_records[0], &def.methods[1]).unwrap();
        assert_eq!(
            herd_loss(&rules, &gone, &founders, &prices).unwrap(),
            Fraction::integer(0)
        );
        prices[crate::tests::good_at(&rules, "cattle")] = 240;
        farm.levels = 3;
        farm.units = 15;
        farm.method_levels = vec![2, 1, 0, 0];
        farm.level_records = (0..3)
            .map(|n| crate::tests::level(n, 1, None, 13))
            .collect();
        farm.level_records[0].method = 0;
        farm.level_records[0].ground = Some("pasture".into());
        farm.level_records[1].method = 0;
        farm.level_records[1].ground = Some("pasture".into());
        farm.herds.fill(0);
        let (heads, _, founders, _) =
            switching_heads(&rules, farm, def, &farm.level_records[2], &def.methods[0]).unwrap();
        assert_eq!(heads[cattle], 6);
        assert_eq!(founders[cattle], 6);
        assert_eq!(
            herd_loss(
                &rules,
                &vec![0; rules.herd_species.len()],
                &founders,
                &prices
            )
            .unwrap(),
            Fraction::integer(1440)
        );
        farm.herds[cattle] = 1;
        let (_, _, founders, _) =
            switching_heads(&rules, farm, def, &farm.level_records[2], &def.methods[0]).unwrap();
        assert_eq!(founders[cattle], 0);
    }

    #[test]
    fn common_herd_does_not_raise_the_margin_and_gain_is_marginal() {
        let stay = Fraction::integer(100);
        let switch = Fraction::integer(150);
        let common = Fraction::integer(500);
        let lost = Fraction::integer(20);
        let staying = stay.add(&common.saturating_sub(&common));
        let switching = switch.add(&common.saturating_sub(&common));
        let threshold = staying
            .add(&staying.mul(&Fraction::ratio(200, 1000)))
            .add(&lost);
        assert_eq!(
            SignedFraction::difference(&switching, &threshold).magnitude,
            Fraction::integer(10)
        );
        assert!(SignedFraction::difference(&switching, &threshold).positive());
    }

    #[test]
    fn fuel_fills_its_job_before_the_remaining_heat() {
        let mut rules = crate::tests::tiny_rules();
        rules.goods[0].id = "peat".into();
        rules.goods[0].tags = vec!["fuel".into()];
        rules.goods[0].supplies = BTreeMap::from([("fuel".into(), 700)]);
        let want = Wanted {
            goods: vec![Fraction::integer(0)],
            jobs: vec![Fraction::integer(1)],
            heat: Fraction::ratio(7, 10),
        };
        assert_eq!(
            value(&rules, &[Fraction::integer(1)], &want, &[100]),
            Fraction::integer(100)
        );
        assert_eq!(
            value(&rules, &[Fraction::integer(2)], &want, &[100]),
            Fraction::integer(200)
        );
        assert_eq!(
            value(&rules, &[Fraction::integer(3)], &want, &[100]),
            Fraction::integer(200)
        );
        rules.goods[0].supplies.clear();
        let default_heat = Wanted {
            goods: vec![Fraction::integer(0)],
            jobs: vec![Fraction::integer(1)],
            heat: Fraction::integer(1),
        };
        assert_eq!(
            value(&rules, &[Fraction::integer(2)], &default_heat, &[100]),
            Fraction::integer(200)
        );
    }

    #[test]
    fn rational_arithmetic_reduces_and_compares_without_overflow() {
        let half = Fraction::ratio(1, 2);
        let third = Fraction::ratio(1, 3);
        assert_eq!(half.add(&third), Fraction::ratio(5, 6));
        assert_eq!(half.sub(&third), Some(Fraction::ratio(1, 6)));
        assert_eq!(
            SignedFraction::difference(&third, &half).magnitude,
            Fraction::ratio(1, 6)
        );
        let big = Fraction::new(Big::new(u128::MAX).mul(&Big::new(u128::MAX)), Big::new(3));
        assert_eq!(
            big.mul(&Fraction::ratio(3, 1)),
            Fraction::new(Big::new(u128::MAX).mul(&Big::new(u128::MAX)), Big::new(1))
        );
    }
}

#[cfg(test)]
mod switch_step_tests {
    use super::*;
    use crate::tests::{farm_with, good_at, level, level_rules, shipped_production_fixture};
    use crate::{
        EconomyState, HerdYield, derive_level_sums, run_monthly_production,
        run_monthly_production_switching, validate_level_records,
    };
    use ck_sim::Canonical;

    const YEAR: u16 = 768;

    fn on(month: u8, year: u16) -> Date {
        Date {
            year,
            month,
            day: 1,
            hour: 12,
        }
    }

    /// The March fixture's rules and place, with the demand carried by a population that wants 100 oats and all the fuel it can
    /// get, since the monthly step has no jobs: a level's oats are worth 100 a year to it, and its wood 600.
    fn march() -> (Rules, EconomyState, LandCounts) {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        rules
            .seasons
            .insert("grain".into(), vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0]);
        rules.seasons.insert(
            "hardwood".into(),
            vec![250, 250, 0, 0, 0, 0, 0, 0, 0, 0, 250, 250],
        );
        let oats = good_at(&rules, "oats");
        rules.goods[oats].hunger_centi = 36_500;
        rules
            .diet_profiles
            .insert("peasant".into(), BTreeMap::from([("oats".into(), 1000)]));
        rules
            .pop_types
            .iter_mut()
            .find(|p| p.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 1000;
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.herds.clear();
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 100)]);
        def.methods[2].annual_output = BTreeMap::from([("hardwood".into(), 600)]);
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        place.pops[0].workers = 100;
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10), ("forest".into(), 8)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 0,
        };
        (rules, state, land)
    }

    fn step_at(
        rules: &Rules,
        state: &mut EconomyState,
        land: &LandCounts,
        date: Date,
        prices: u64,
    ) -> crate::ProductionLedger {
        let day = vec![vec![prices; rules.goods.len()]];
        run_monthly_production_switching(
            rules,
            state,
            date,
            &BTreeMap::new(),
            &BTreeMap::from([("fixture".to_owned(), land.clone())]),
            &day,
        )
        .unwrap()
    }

    fn step(
        rules: &Rules,
        state: &mut EconomyState,
        land: &LandCounts,
        date: Date,
    ) -> crate::ProductionLedger {
        step_at(rules, state, land, date, 1)
    }

    fn held(state: &EconomyState, good: usize) -> u64 {
        let place = &state.places[0];
        place.pops.iter().map(|p| p.stock[good]).sum::<u64>()
            + place.buildings.iter().map(|b| b.stock[good]).sum::<u64>()
    }

    fn methods(state: &EconomyState, building: usize) -> Vec<u32> {
        state.places[0].buildings[building]
            .level_records
            .iter()
            .map(|record| record.method)
            .collect()
    }

    #[test]
    fn a_non_growing_move_is_mature_and_drops_old_accounts() {
        let (rules, mut state, land) = march();
        let oats = good_at(&rules, "oats");
        let wood = good_at(&rules, "hardwood");
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), Big::new(50 * 144_000));
        let mut baseline = state.clone();
        run_monthly_production(&rules, &mut baseline, on(3, YEAR)).unwrap();
        step(&rules, &mut state, &land, on(3, YEAR));
        let farm = &state.places[0].buildings[0];
        assert_eq!(
            farm.level_records,
            vec![LevelRecord {
                number: 0,
                method: 2,
                ground: Some("forest".into()),
                paydays: 13,
                unpaid_work: BTreeMap::from([("hardwood".to_owned(), Big::default())]),
                last_switch: Some((YEAR, 3)),
            }]
        );
        assert_eq!(farm.method_levels, vec![0, 0, 1, 0]);
        let def = definition(&rules, farm).unwrap();
        assert_eq!(
            (
                farm.method_suitability.clone(),
                farm.herd_grazing_sums.clone()
            ),
            derive_level_sums(&rules, def, &farm.level_records).unwrap()
        );
        // The building's levels, workers, carries, wage history and stores are as the month's production left them.
        let base = &baseline.places[0].buildings[0];
        assert_eq!(
            (
                farm.levels,
                &farm.employment,
                &farm.production_carries,
                &farm.wage_history,
                &farm.paid_history,
                &farm.paid_years,
                &farm.wage_carries,
                &farm.pay_carries,
                &farm.stock,
            ),
            (
                base.levels,
                &base.employment,
                &base.production_carries,
                &base.wage_history,
                &base.paid_history,
                &base.paid_years,
                &base.wage_carries,
                &base.pay_carries,
                &base.stock,
            )
        );
        // What the level had worked for and not been paid is gone, and nothing was paid for it.
        assert_eq!(held(&state, oats), held(&baseline, oats));
        validate_level_records(&rules, &BTreeMap::new(), &state).unwrap();
        // The level is paid nothing on 1 April, and in November seven twelfths of its month of wood; it does not move again.
        let mut paid = Vec::new();
        for month in 4..=12_u8 {
            let ledger = step(&rules, &mut state, &land, on(month, YEAR));
            let record = &state.places[0].buildings[0].level_records[0];
            assert_eq!(
                (record.method, record.last_switch, record.paydays),
                (2, Some((YEAR, 3)), 13)
            );
            assert_eq!(ledger.produced[oats], 0, "month {month}: no crop");
            paid.push(ledger.produced[wood]);
        }
        assert_eq!(paid[0], 0, "April");
        assert_eq!(paid[7], 150, "November pays the full mature share");
        assert_eq!(paid[8], 150, "December pays the full mature share");
    }

    #[test]
    fn a_level_that_switched_this_year_is_not_tried_and_is_tried_again_in_the_next() {
        let (rules, state, land) = march();
        let prices = vec![1; rules.goods.len()];
        for (switched, tried) in [
            (Some((YEAR, 3)), false),
            (Some((YEAR - 1, 12)), true),
            (None, true),
        ] {
            let mut case = state.clone();
            case.places[0].buildings[0].level_records[0].last_switch = switched;
            let looked = look_in_year(
                &rules,
                &case.places[0],
                &land,
                0,
                3,
                Some(YEAR),
                &prices,
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(!looked.moves.is_empty(), tried, "{switched:?}");
            step(&rules, &mut case, &land, on(3, YEAR));
            assert_eq!(methods(&case, 0) == vec![2], tried, "{switched:?}");
            let record = &case.places[0].buildings[0].level_records[0];
            assert_eq!(
                record.last_switch,
                if tried { Some((YEAR, 3)) } else { switched }
            );
        }
    }

    #[test]
    fn a_switch_unlocks_next_month_and_across_december() {
        let (rules, mut state, land) = march();
        let prices = vec![1; rules.goods.len()];
        for (stamp, year, month, eligible) in [
            ((YEAR, 5), YEAR, 5, false),
            ((YEAR, 5), YEAR, 6, true),
            ((YEAR, 12), YEAR + 1, 1, true),
        ] {
            state.places[0].buildings[0].level_records[0].last_switch = Some(stamp);
            let result = look_in_year(
                &rules,
                &state.places[0],
                &land,
                0,
                month,
                Some(year),
                &prices,
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(
                !result.moves.is_empty(),
                eligible,
                "{stamp:?} at {year}-{month}"
            );
        }
    }

    #[test]
    fn equal_size_look_offers_both_records_and_prefers_the_lower_unpaid_account() {
        let (rules, mut state, mut land) = march();
        land.land_cells.insert("forest".into(), 8);
        farm_with(
            &rules,
            &mut state,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("pasture"), 13),
            ],
        );
        state.places[0].buildings[0].herds.fill(0);
        state.places[0].buildings[0].level_records[1]
            .unpaid_work
            .insert("oats".into(), Big::new(50 * 144_000));
        let prices = vec![1; rules.goods.len()];
        let result = look_in_year(
            &rules,
            &state.places[0],
            &land,
            0,
            3,
            Some(YEAR),
            &prices,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            result
                .moves
                .iter()
                .any(|entry| entry.method == 2 && entry.level == 0)
        );
        assert!(
            result
                .moves
                .iter()
                .any(|entry| entry.method == 2 && entry.level == 1)
        );
        let best = &result.moves[result.answer.unwrap()];
        assert_eq!((best.method, best.level), (2, 0));
    }

    #[test]
    fn two_levels_that_pass_move_one_a_month_the_larger_surplus_first() {
        let (rules, mut state, mut land) = march();
        land.land_cells.insert("forest".into(), 8);
        farm_with(
            &rules,
            &mut state,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("pasture"), 13),
            ],
        );
        state.places[0].buildings[0].herds.fill(0);
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), Big::new(50 * 144_000));
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!(
            methods(&state, 0),
            vec![0, 2],
            "level 1, with nothing unpaid, has the larger surplus"
        );
        step(&rules, &mut state, &land, on(4, YEAR));
        assert_eq!(
            methods(&state, 0),
            vec![2, 2],
            "level 0 is looked at again in April, with level 1 among the other levels"
        );
        let years: Vec<_> = state.places[0].buildings[0]
            .level_records
            .iter()
            .map(|record| record.last_switch)
            .collect();
        assert_eq!(years, vec![Some((YEAR, 4)), Some((YEAR, 3))]);
    }

    /// Two farms in one place, the earlier first, each with one level at sea and fish nobody wants.
    fn two_farms(rooms: u64) -> (Rules, EconomyState, LandCounts) {
        let (mut rules, mut state, mut land) = march();
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.methods[1].annual_output = BTreeMap::from([("fish".into(), 1)]);
        farm_with(&rules, &mut state, vec![level(0, 1, None, 13)]);
        let mut second = state.places[0].buildings[0].clone();
        second.employment[0].workers = 10;
        state.places[0].buildings.push(second);
        state.places[0].pops[0].workers = 200;
        // Rent room: each land level of a farm costs 2,000.
        land.fertility_pm = BTreeMap::from([("pasture".into(), rooms * 2000)]);
        land.land_cells.remove("forest");
        land.fertile_cells = 10;
        (rules, state, land)
    }

    #[test]
    fn the_earlier_buildings_peat_is_all_the_place_wants_and_the_later_look_finds_it_unwanted() {
        // Both farms have a level that passes for turf and both have the room, but the place wants one level's peat: the
        // earlier building's level moves, and the later look, which sees its new output, finds peat nearly unwanted.
        let (mut rules, mut state, mut land) = march();
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        // This synthetic method grows, to exercise the slow-output forecast.
        def.methods[3].grows = true;
        def.methods[3].annual_output = BTreeMap::from([("peat".into(), 600)]);
        rules
            .pop_types
            .iter_mut()
            .find(|p| p.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 100;
        let mut second = state.places[0].buildings[0].clone();
        second.employment[0].workers = 10;
        state.places[0].buildings.push(second);
        // Four people burn 146 of heat a year. A level that moves to turf at 1 March is paid 225 of peat in its first year (the
        // slow start), which gives 157 of heat: the first level's coming output is all the place wants.
        state.places[0].pops[0].workers = 4;
        state.places[0].buildings[0].employment[0].workers = 10;
        land.land_cells = BTreeMap::from([("pasture".into(), 10), ("bog".into(), 8)]);
        land.fertility_pm = BTreeMap::from([("pasture".into(), 10_000)]);
        let mut roomy = state.clone();
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!(
            (methods(&state, 0), methods(&state, 1)),
            (vec![3], vec![0]),
            "the earlier building's level moves and the later one stays"
        );
        // With the place wanting twice the heat both levels move: it is the want, not the room, that stopped the second.
        roomy.places[0].pops[0].workers = 8;
        step(&rules, &mut roomy, &land, on(3, YEAR));
        assert_eq!((methods(&roomy, 0), methods(&roomy, 1)), (vec![3], vec![3]));
    }

    #[test]
    fn the_earlier_building_takes_the_last_field_and_the_later_has_no_room_to_try() {
        let (rules, mut state, land) = two_farms(1);
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!(
            methods(&state, 0),
            vec![0],
            "the earlier building's level takes the field"
        );
        assert_eq!(
            methods(&state, 1),
            vec![1],
            "the later building has no room to try"
        );
        let (rules, mut state, land) = two_farms(2);
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!(
            (methods(&state, 0), methods(&state, 1)),
            (vec![0], vec![0]),
            "with room for both, both move"
        );
        let (rules, mut state, land) = two_farms(0);
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!(
            (methods(&state, 0), methods(&state, 1)),
            (vec![1], vec![1]),
            "a place already out of room: nothing moves"
        );
    }

    #[test]
    fn two_wood_methods_of_different_names_share_the_forests_cells() {
        // A second building type that is the farm under another name, with its wood method under another name too: the two take
        // cells from the same forest, and the earlier building gets the last four.
        let (mut rules, mut state, mut land) = march();
        let mut other = rules
            .buildings
            .iter()
            .find(|b| b.id == "farm")
            .unwrap()
            .clone();
        other.id = "farm2".into();
        other.methods[2].id = "wood_focus2".into();
        rules.buildings.push(other);
        rules
            .footprint_m2_per_unit
            .insert("farm2".into(), rules.footprint_m2_per_unit["farm"]);
        rules
            .housing_class
            .insert("farm2".into(), rules.housing_class["farm"].clone());
        rules
            .housing_milli_per_unit
            .insert("farm2".into(), rules.housing_milli_per_unit["farm"]);
        let mut second = state.places[0].buildings[0].clone();
        second.kind = "farm2".into();
        state.places[0].buildings.push(second);
        state.places[0].pops[0].workers = 200;
        land.land_cells.insert("forest".into(), 4);
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!(
            (methods(&state, 0), methods(&state, 1)),
            (vec![2], vec![0]),
            "four forest cells are one wood level, whatever its method is called"
        );
        // With eight cells both move.
        let mut roomy = state.clone();
        for building in &mut roomy.places[0].buildings {
            let record = &mut building.level_records[0];
            record.method = 0;
            record.ground = Some("pasture".into());
            record.last_switch = None;
            record.paydays = 13;
            building.method_levels = vec![1, 0, 0, 0];
            building.method_suitability = state.places[0].buildings[1].method_suitability.clone();
        }
        let def = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        for building in &mut roomy.places[0].buildings {
            (building.method_suitability, building.herd_grazing_sums) =
                derive_level_sums(&rules, def, &building.level_records).unwrap();
        }
        land.land_cells.insert("forest".into(), 8);
        step(&rules, &mut roomy, &land, on(3, YEAR));
        assert_eq!((methods(&roomy, 0), methods(&roomy, 1)), (vec![2], vec![2]));
    }

    /// The surplus the look finds for the level's move to wood on this state, as (negative, numerator, denominator).
    fn wood_surplus(
        rules: &Rules,
        state: &EconomyState,
        land: &LandCounts,
        month: u8,
    ) -> (bool, u128, u128) {
        let looked = look_in_year(
            rules,
            &state.places[0],
            land,
            0,
            month,
            Some(YEAR),
            &vec![1; rules.goods.len()],
            &BTreeMap::new(),
        )
        .unwrap();
        let item = looked.moves.iter().find(|m| m.method == 2).unwrap();
        (
            item.surplus.negative,
            item.surplus.magnitude.numerator.to_u128().unwrap(),
            item.surplus.magnitude.denominator.to_u128().unwrap(),
        )
    }

    #[test]
    fn a_level_waits_for_the_harvest_is_paid_its_crop_and_then_moves() {
        // Wood is worth 600 from 1 August. The level has worked 13,200,000 144,000ths of oats (91 and two thirds) and not been
        // paid, and in September it is paid its whole crop.
        let (mut rules, mut state, land) = march();
        // A growing candidate keeps the slow-start cost in this harvest witness.
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[2]
            .grows = true;
        let oats = good_at(&rules, "oats");
        // The month's production adds to the account, so what is seeded is what makes the account 13,200,000 afterwards.
        let mut probe = state.clone();
        run_monthly_production(&rules, &mut probe, on(8, YEAR)).unwrap();
        let worked = probe.places[0].buildings[0].level_records[0].unpaid_work["oats"]
            .to_u128()
            .unwrap();
        assert!(worked < 13_200_000);
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), Big::new(13_200_000 - worked));
        step(&rules, &mut state, &land, on(8, YEAR));
        assert_eq!(methods(&state, 0), vec![0], "no move on 1 August");
        assert_eq!(
            state.places[0].buildings[0].level_records[0].unpaid_work["oats"],
            Big::new(13_200_000)
        );
        assert_eq!(
            wood_surplus(&rules, &state, &land, 8),
            (true, 110, 3),
            "the surplus is 110 over 3 below the line"
        );
        // On 1 September the level is paid its whole crop, and then, with nothing unpaid and a slow start of 475, moves.
        let mut paid = state.clone();
        let ledger = run_monthly_production(&rules, &mut paid, on(9, YEAR)).unwrap();
        assert_eq!(ledger.produced[oats], 100, "the whole crop is paid");
        assert_eq!(
            paid.places[0].buildings[0].level_records[0].unpaid_work["oats"],
            Big::default()
        );
        assert_eq!(wood_surplus(&rules, &paid, &land, 9), (false, 5, 1));
        let ledger = step(&rules, &mut state, &land, on(9, YEAR));
        assert_eq!(ledger.produced[oats], 100, "the crop is paid once");
        assert_eq!(methods(&state, 0), vec![2], "then the level moves");
        assert_eq!(held(&state, oats), held(&paid, oats));
    }

    #[test]
    fn a_field_on_forest_ground_gives_back_its_room_and_takes_forest_cells_counted_apart() {
        // The only ground is forest: room for one field level (2,000) and four cells, one wood level. A second building's sea level
        // wants the field. It has no room while the first building's level is on it, and has it when that level has moved to
        // the wood focus, which takes forest cells and no field room.
        let (mut rules, mut state, mut land) = march();
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[1]
            .annual_output = BTreeMap::from([("fish".into(), 1)]);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("forest"), 13)]);
        let mut second = state.places[0].buildings[0].clone();
        second.employment[0].workers = 10;
        {
            let record = &mut second.level_records[0];
            record.method = 1;
            record.ground = None;
            record.unpaid_work = BTreeMap::new();
            second.method_levels = vec![0, 1, 0, 0];
        }
        let def = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        (second.method_suitability, second.herd_grazing_sums) =
            derive_level_sums(&rules, def, &second.level_records).unwrap();
        state.places[0].buildings.push(second);
        state.places[0].pops[0].workers = 200;
        land.land_cells = BTreeMap::from([("forest".into(), 4)]);
        land.fertility_pm = BTreeMap::from([("forest".into(), 2000)]);
        land.fertile_cells = 4;
        // Before the move the field holds the room: the second building cannot try the field.
        let blocked = look_in_year(
            &rules,
            &state.places[0],
            &land,
            1,
            3,
            Some(YEAR),
            &vec![1; rules.goods.len()],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(blocked.moves.iter().all(|m| m.method != 0));
        step(&rules, &mut state, &land, on(3, YEAR));
        let first = &state.places[0].buildings[0];
        assert_eq!(first.level_records[0].method, 2);
        assert_eq!(first.level_records[0].ground.as_deref(), Some("forest"));
        assert_eq!(first.method_levels, vec![0, 0, 1, 0]);
        // The room it gave back went to the later building at once, on the same ground; its four cells are the wood level's.
        let later = &state.places[0].buildings[1];
        assert_eq!(later.level_records[0].method, 0);
        assert_eq!(later.level_records[0].ground.as_deref(), Some("forest"));
        assert_eq!(later.method_levels, vec![1, 0, 0, 0]);
    }

    #[test]
    fn the_sea_is_not_used_up_by_a_move_to_it_and_needs_one_sea_cell() {
        let (mut rules, mut state, mut land) = march();
        let fish = good_at(&rules, "fish");
        rules.goods[fish].hunger_centi = 36_500;
        rules
            .diet_profiles
            .insert("peasant".into(), BTreeMap::from([("fish".into(), 1000)]));
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.methods[1].annual_output = BTreeMap::from([("fish".into(), 100)]);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        let mut second = state.places[0].buildings[0].clone();
        second.employment[0].workers = 10;
        state.places[0].buildings.push(second);
        state.places[0].pops[0].workers = 200;
        land.land_cells.remove("forest");
        // With no sea cell in the catchment neither level is tried at sea and neither moves.
        step(&rules, &mut state.clone(), &land, on(3, YEAR));
        let mut dry = state.clone();
        step(&rules, &mut dry, &land, on(3, YEAR));
        assert_eq!((methods(&dry, 0), methods(&dry, 1)), (vec![0], vec![0]));
        // With one sea cell the earlier building's level moves, and the later building's look still finds the sea open.
        land.sea = 1;
        step(&rules, &mut state, &land, on(3, YEAR));
        assert_eq!((methods(&state, 0), methods(&state, 1)), (vec![1], vec![1]));
        for building in &state.places[0].buildings {
            assert_eq!(building.level_records[0].ground, None);
        }
        assert_eq!(land.sea, 1);
    }

    /// A farm of ten levels on mixed farming with a herd of cattle that gives three goods, fish wanted so that a level passes
    /// for the sea, no births or deaths, and prices that make a posted wage show.
    fn culling(head: u64) -> (Rules, EconomyState, LandCounts, usize) {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        for species in &mut rules.herd_species {
            species.births_pm_year = 0;
            species.deaths_pm_year = 0;
        }
        let cattle = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        rules.herd_species[cattle].yields.push(HerdYield {
            good: "mutton".into(),
            units: 5,
        });
        let fish = good_at(&rules, "fish");
        rules.goods[fish].hunger_centi = 36_500;
        rules
            .diet_profiles
            .insert("peasant".into(), BTreeMap::from([("fish".into(), 1000)]));
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.herds.retain(|name, _| name == "cattle");
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 100)]);
        def.methods[1].annual_output = BTreeMap::from([("fish".into(), 8000)]);
        let mut state = shipped_production_fixture(&rules);
        farm_with(
            &rules,
            &mut state,
            (0..10).map(|n| level(n, 0, Some("pasture"), 13)).collect(),
        );
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        place.buildings[0].herds[cattle] = head;
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        place.pops[0].workers = 20_000;
        place.prices_milli = vec![1000; rules.goods.len()];
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 40)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 40_000)]),
            fertile_cells: 40,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 40)]),
            sea: 1,
        };
        (rules, state, land, cattle)
    }

    #[test]
    fn a_herd_that_no_longer_fits_is_slaughtered_that_day_and_paid_by_paydays_own_split() {
        let (rules, state, land, cattle) = culling(118);
        let (beef, hides, mutton, oats) = (
            good_at(&rules, "beef"),
            good_at(&rules, "hides"),
            good_at(&rules, "mutton"),
            good_at(&rules, "oats"),
        );
        let september = on(9, YEAR);
        let mut baseline = state.clone();
        let paid = run_monthly_production(&rules, &mut baseline, september).unwrap();
        let mut moved = state.clone();
        let ledger = step_at(&rules, &mut moved, &land, september, 1000);
        // One level moved to fishing, and ten head went: 118 and nine levels' 108.
        assert_eq!(methods(&moved, 0).iter().filter(|m| **m == 1).count(), 1);
        assert_eq!(moved.places[0].buildings[0].herds[cattle], 108);
        assert_eq!(ledger.herd_slaughter[cattle], 10);
        assert_eq!(ledger.places[0].herd_slaughter[cattle], 10);
        assert_eq!(ledger.herd_arrivals[cattle], 0);
        // Beef, hides and a third yield are paid out that day, every one of them output of the building.
        for (good, units) in [(beef, 2200), (hides, 10), (mutton, 50)] {
            assert_eq!(ledger.produced[good], paid.produced[good] + units, "{good}");
            assert_eq!(
                held(&moved, good) + ledger.lost[good],
                held(&baseline, good) + paid.lost[good] + units,
                "{good}"
            );
        }
        // The crop was paid once at payday and not again.
        assert_eq!(ledger.produced[oats], paid.produced[oats]);
        // Payday's split, carried on: the posted wage, paid heads and year are what payday posted.
        let after = &moved.places[0].buildings[0];
        let before = &baseline.places[0].buildings[0];
        assert_eq!(
            (&after.wage_history, &after.paid_history, &after.paid_years),
            (
                &before.wage_history,
                &before.paid_history,
                &before.paid_years
            )
        );
        // The farm's overhead is a tenth: 900 of every thousand go to wages, the rest is passed on.
        assert_eq!(after.wage_carries[beef], before.wage_carries[beef]);
        assert_eq!(after.pay_carries[beef], 0);
        // The herd adds up for the pass, for the place and kind.
        let place = &ledger.places[0];
        assert_eq!(
            place.herds_before[cattle] + place.herd_births[cattle] + place.herd_arrivals[cattle],
            place.herds_after[cattle] + place.herd_deaths[cattle] + place.herd_slaughter[cattle]
        );
        // With 100 head nothing is slaughtered.
        let (rules, mut small, land, cattle) = culling(100);
        let ledger = step_at(&rules, &mut small, &land, september, 1000);
        assert_eq!(small.places[0].buildings[0].herds[cattle], 100);
        assert_eq!(ledger.herd_slaughter[cattle], 0);
        assert_eq!(ledger.produced[beef], 0);
    }

    #[test]
    fn a_cull_is_not_a_second_payday() {
        let (rules, state, land, _) = culling(118);
        let beef = good_at(&rules, "beef");
        let september = on(9, YEAR);
        let mut baseline = state.clone();
        run_monthly_production(&rules, &mut baseline, september).unwrap();
        let mut moved = state.clone();
        step_at(&rules, &mut moved, &land, september, 1000);
        let (after, before) = (
            &moved.places[0].buildings[0],
            &baseline.places[0].buildings[0],
        );
        // A second payday would post a higher wage and pay the crop twice; the paid heads and year are payday's own.
        assert_eq!(after.wage_history[8], before.wage_history[8]);
        assert_eq!(after.paid_history[8], before.paid_history[8]);
        assert_eq!(after.paid_years[8], YEAR);
        assert_eq!(after.production_carries, before.production_carries);
        assert_eq!(
            after.last_made, before.last_made,
            "a switch cull is not payday output"
        );
        // The beef's wages (900 of every thousand, by the farm's overhead) go to the workers' stores, and what does not fit and
        // the overhead are passed on; none of it is lost or kept back in a carry.
        assert_eq!(after.wage_carries[beef], 0);
        assert_eq!(after.pay_carries[beef], 0);
        assert_eq!(held(&moved, beef), held(&baseline, beef) + 2200);
    }

    /// The cull fixture made ready for the whole daily step: valid price, pop and employment dimensions.
    fn daily_culling(head: u64) -> (Rules, EconomyState, LandCounts, usize) {
        let (rules, mut state, land, cattle) = culling(head);
        let place = &mut state.places[0];
        place.price_drift_carries = vec![0; rules.goods.len()];
        for pop in &mut place.pops {
            pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            pop.consumption.ticking_carries = vec![0; rules.goods.len()];
            pop.employment.truncate(1);
            pop.consumption.necessities_pm = 1000;
            pop.consumption.niceties_pm = 1000;
            pop.consumption.luxuries_pm = 1000;
        }
        place.pops[0].workers = 24_000;
        place.pops[0].employment = vec![place.buildings[0].employment[0].clone()];
        (rules, state, land, cattle)
    }

    #[test]
    fn a_cull_is_paid_by_paydays_split_to_each_recipient_with_the_carry_taken_on() {
        // The farm keeps a tenth and a half of a tenth of its output for overhead (105 per mille), so a wage is 895 of every
        // thousand. Its workers are all of the first pop. The mutton's wage carry stood at 400,000 millionths after payday.
        let (mut rules, mut state, land, _) = culling(118);
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .overhead_pm = 105;
        let (beef, mutton, hides) = (
            good_at(&rules, "beef"),
            good_at(&rules, "mutton"),
            good_at(&rules, "hides"),
        );
        state.places[0].buildings[0].wage_carries[mutton] = 400_000;
        let mut baseline = state.clone();
        run_monthly_production(&rules, &mut baseline, on(9, YEAR)).unwrap();
        let mut moved = state;
        let ledger = step_at(&rules, &mut moved, &land, on(9, YEAR), 1000);
        let farm = &moved.places[0].buildings[0];
        let before = &baseline.places[0].buildings[0];
        let pop_stock = |state: &EconomyState, good: usize| -> Vec<u64> {
            state.places[0].pops.iter().map(|p| p.stock[good]).collect()
        };
        // (good, the first pop's stock, the farm's stock, the wage carry and the pay carry): 2,200 beef is a wage of 1,969 and
        // 231 of overhead; 50 mutton and the old carry make a wage of 45, 5 of overhead and a carry of 150,000; 10 hides make
        // a wage of 8, 2 of overhead and a carry of 950,000. Nothing is lost.
        for (good, wage, overhead, carry) in [
            (beef, 1969, 231, 0),
            (mutton, 45, 5, 150_000),
            (hides, 8, 2, 950_000),
        ] {
            assert_eq!(pop_stock(&moved, good), vec![wage, 0, 0], "{good}");
            assert_eq!(pop_stock(&baseline, good), vec![0, 0, 0], "{good}");
            assert_eq!(farm.stock[good], overhead, "{good}");
            assert_eq!(before.stock[good], 0, "{good}");
            assert_eq!(ledger.lost[good], 0, "{good}");
            assert_eq!(farm.wage_carries[good], carry, "{good}");
            assert_eq!(farm.pay_carries[good], 0, "{good}");
        }
        // The posted pay is what payday posted.
        assert_eq!(
            (&farm.wage_history, &farm.paid_history, &farm.paid_years),
            (
                &before.wage_history,
                &before.paid_history,
                &before.paid_years
            )
        );
    }

    #[test]
    fn a_cull_that_overflows_the_workers_store_is_routed_to_the_farm_and_lost_beyond_it() {
        // Forty workers hold up to 30 lb of beef each, 1,200 lb: 1,000 are held, so there is room for 200. The farm's store takes
        // 4 lb for each of its 100 workers, 400 lb. No other pop has anyone to hold anything. The cull of 2,200 lb of beef is a
        // wage of 1,969 and 231 of overhead (105 per mille):
        //   overhead 231 goes to the farm's store, which then has room for 169;
        //   the wage 1,969 goes to the workers, who take 200 (their room); the 1,769 over goes to the buildings the pop owns a share
        //   of, the farm, which takes 169; and the 1,600 that is left is lost.
        let (mut rules, mut state, land, _) = culling(118);
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .overhead_pm = 105;
        let beef = good_at(&rules, "beef");
        state.places[0].pops[0].workers = 40;
        state.places[0].pops[0].stock[beef] = 1_000;
        let mut baseline = state.clone();
        run_monthly_production(&rules, &mut baseline, on(9, YEAR)).unwrap();
        let mut moved = state;
        let ledger = step_at(&rules, &mut moved, &land, on(9, YEAR), 1000);
        let place = &moved.places[0];
        assert_eq!(
            place.pops.iter().map(|p| p.stock[beef]).collect::<Vec<_>>(),
            vec![1_200, 0, 0],
            "the workers' store is full"
        );
        assert_eq!(
            place.buildings[0].stock[beef], 400,
            "the farm's store is full"
        );
        assert_eq!(ledger.lost[beef], 1_600);
        assert_eq!(place.counters.current.lost[beef], 1_600);
        assert_eq!(place.pops[0].counters.current.losses, 1_600);
        assert_eq!(ledger.produced[beef], 2_200);
        // Every unit is somewhere: what the stores gained and what was lost make the yield.
        assert_eq!(held(&moved, beef) - 1_000 + ledger.lost[beef], 2_200);
        // The wage carry stands and the posted pay is what payday posted.
        let farm = &place.buildings[0];
        let before = &baseline.places[0].buildings[0];
        assert_eq!((farm.wage_carries[beef], farm.pay_carries[beef]), (0, 0));
        assert_eq!(
            (&farm.wage_history, &farm.paid_history, &farm.paid_years),
            (
                &before.wage_history,
                &before.paid_history,
                &before.paid_years
            )
        );
    }

    #[test]
    fn a_cull_and_the_same_days_emergency_butchering_are_two_ledgers() {
        let (rules, mut state, land, cattle) = daily_culling(118);
        state.places[0].pops[0].consumption.necessities_pm = 400;
        state.places[0].pops[0].workers = 60_000;
        let lands = BTreeMap::from([("fixture".to_owned(), land)]);
        let mut day = state.clone();
        let ledger = crate::run_daily_economy_with_land(
            &rules,
            &mut day,
            on(9, YEAR),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &lands,
        )
        .unwrap();
        // The production pass slaughtered the ten of the switch, and its herds add up without what the pop butchered after.
        assert_eq!(ledger.herd_slaughter[cattle], 10);
        let place = &ledger.places[0];
        assert_eq!(place.herds_before[cattle], 118);
        assert_eq!(place.herds_after[cattle], 108);
        assert_eq!(
            place.herds_before[cattle] + place.herd_births[cattle] + place.herd_arrivals[cattle],
            place.herds_after[cattle] + place.herd_deaths[cattle] + place.herd_slaughter[cattle]
        );
        // The day's own ledger has the pop's butchering after it, and starts from the 108.
        let eating = ledger.daily_consumption.as_ref().unwrap();
        assert_eq!(eating.herds_before[cattle], 108);
        // The hungry owner needs another 79 cattle after the switching cull; the two ledgers count them separately.
        assert_eq!(eating.herd_slaughter[cattle], 79);
        assert_eq!(eating.herds_after[cattle], 29);
        assert_eq!(
            day.places[0].buildings[0].herds[cattle],
            eating.herds_after[cattle]
        );
    }

    #[test]
    fn founders_arrive_in_a_herd_that_has_none_and_are_not_births() {
        let (rules, mut state, mut land, cattle) = culling(0);
        // Every level at sea and no animals; the place wants oats now, and a level moves to farming.
        let oats = good_at(&rules, "oats");
        let mut rules = rules;
        // Oats are worth far more than the fish a sea level has worked for and not been paid.
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[0]
            .annual_output = BTreeMap::from([("oats".into(), 10_000)]);
        rules.goods[oats].hunger_centi = 36_500;
        rules
            .diet_profiles
            .insert("peasant".into(), BTreeMap::from([("oats".into(), 1000)]));
        let farm = &mut state.places[0].buildings[0];
        for record in &mut farm.level_records {
            record.method = 1;
            record.ground = None;
        }
        farm.method_levels = vec![0, 10, 0, 0];
        let def = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        (farm.method_suitability, farm.herd_grazing_sums) =
            derive_level_sums(&rules, def, &farm.level_records).unwrap();
        land.sea = 1;
        let before = held(&state, oats);
        let ledger = step_at(&rules, &mut state, &land, on(3, YEAR), 1000);
        assert_eq!(
            methods(&state, 0).iter().filter(|m| **m == 0).count(),
            1,
            "a level moved to farming"
        );
        // Two head for each of the building's levels that now keep animals: one level, two head.
        assert_eq!(state.places[0].buildings[0].herds[cattle], 2);
        assert_eq!(ledger.herd_arrivals[cattle], 2);
        assert_eq!(ledger.herd_births[cattle], 0);
        assert_eq!(held(&state, oats), before, "no store paid for them");
        let place = &ledger.places[0];
        assert_eq!(
            place.herds_before[cattle] + place.herd_births[cattle] + place.herd_arrivals[cattle],
            place.herds_after[cattle] + place.herd_deaths[cattle] + place.herd_slaughter[cattle]
        );
        // A second level moving the year after finds animals and brings none.
        state.places[0].buildings[0].level_records[0].last_switch = None;
        let ledger = step_at(&rules, &mut state, &land, on(3, YEAR + 1), 1000);
        assert_eq!(ledger.herd_arrivals[cattle], 0);
    }

    #[test]
    fn a_founder_herd_is_never_over_its_maximum() {
        let (mut rules, mut state, mut land, cattle) = culling(0);
        let oats = good_at(&rules, "oats");
        // Oats are worth far more than the fish a sea level has worked for and not been paid.
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[0]
            .annual_output = BTreeMap::from([("oats".into(), 10_000)]);
        rules.goods[oats].hunger_centi = 36_500;
        rules
            .diet_profiles
            .insert("peasant".into(), BTreeMap::from([("oats".into(), 1000)]));
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .herds
            .get_mut("cattle")
            .unwrap()
            .maximum = 1;
        let farm = &mut state.places[0].buildings[0];
        for record in &mut farm.level_records {
            record.method = 1;
            record.ground = None;
        }
        farm.method_levels = vec![0, 10, 0, 0];
        let def = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        (farm.method_suitability, farm.herd_grazing_sums) =
            derive_level_sums(&rules, def, &farm.level_records).unwrap();
        land.sea = 1;
        let ledger = step_at(&rules, &mut state, &land, on(3, YEAR), 1000);
        assert_eq!(ledger.herd_arrivals[cattle], 1, "one head, not two");
        assert_eq!(state.places[0].buildings[0].herds[cattle], 1);
    }

    #[test]
    fn a_bog_level_that_moves_to_turf_leaves_the_crop_suitability_to_the_pasture_level() {
        // A farm with one level on pasture and one field on bog ground, where the place wants fuel and peat is cut: the bog
        // level moves to turf, the pasture level (which has worked for more) does not, and the farm's sums follow the records.
        // The farm keeps cattle: 18 head is above one level's maximum of 12 and within two levels', so a turf level that kept no
        // animals would cull six head, and one that does keep them culls none.
        let (mut rules, mut state, mut land) = march();
        for species in &mut rules.herd_species {
            species.births_pm_year = 0;
            species.deaths_pm_year = 0;
        }
        let cattle = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.herds
            .insert("cattle".into(), crate::HerdCapacity { maximum: 12 });
        def.methods[3].annual_output = BTreeMap::from([("peat".into(), 600)]);
        let peat = good_at(&rules, "peat");
        rules
            .seasons
            .insert("peat".into(), vec![0, 0, 0, 0, 0, 0, 0, 500, 500, 0, 0, 0]);
        farm_with(
            &rules,
            &mut state,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("bog"), 13),
            ],
        );
        state.places[0].buildings[0].herds[cattle] = 18;
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), Big::new(500 * 144_000));
        land.land_cells = BTreeMap::from([("pasture".into(), 10), ("bog".into(), 8)]);
        land.fertility_pm = BTreeMap::from([("pasture".into(), 10_000), ("bog".into(), 8_000)]);
        land.fertile_cells = 18;
        let oats = good_at(&rules, "oats");
        let def = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let pasture = u64::from(method_suitability_pm(
            &rules,
            def,
            &def.methods[0],
            "oats",
            Some("pasture"),
        ));
        let both = state.places[0].buildings[0].method_suitability[0][oats];
        assert!(
            both > pasture,
            "the bog level adds to the crop's suitability"
        );
        let ledger = step(&rules, &mut state, &land, on(3, YEAR));
        let farm = &state.places[0].buildings[0];
        assert_eq!(
            farm.level_records
                .iter()
                .map(|r| (r.method, r.ground.as_deref(), r.last_switch))
                .collect::<Vec<_>>(),
            vec![
                (0, Some("pasture"), None),
                (3, Some("bog"), Some((YEAR, 3)))
            ]
        );
        // Both levels still keep animals: the maximum is two levels', and no head is slaughtered or arrives.
        assert_eq!(farm.herds[cattle], 18);
        assert_eq!(ledger.herd_slaughter[cattle], 0);
        assert_eq!(ledger.herd_arrivals[cattle], 0);
        assert_eq!(farm.method_levels, vec![1, 0, 0, 1]);
        // The crop's suitability is the pasture level's alone; the grazing sums are each level's own ground's, the turf
        // level's bog included (CK-406 r3, rule 4), and the herds' maximums count both levels that keep animals.
        assert_eq!(farm.method_suitability[0][oats], pasture);
        assert_eq!(
            farm.method_suitability[3][peat],
            u64::from(method_suitability_pm(
                &rules,
                def,
                &def.methods[3],
                "peat",
                Some("bog")
            ))
        );
        let cattle = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        assert_eq!(
            farm.herd_grazing_sums[cattle],
            u64::from(rules.grazing["pasture"]["cattle"])
                + u64::from(rules.grazing["bog"]["cattle"])
        );
        assert_eq!(
            farm.herd_grazing_sums,
            derive_level_sums(&rules, def, &farm.level_records)
                .unwrap()
                .1
        );
    }

    fn next_day(mut date: Date) -> Date {
        if date.day == ck_sim::days_in_month(date.year, date.month).unwrap() {
            date.day = 1;
            if date.month == 12 {
                date.month = 1;
                date.year += 1;
            } else {
                date.month += 1;
            }
        } else {
            date.day += 1;
        }
        date
    }

    /// The march fixture where nothing but oats is wanted, so that no move ever passes.
    fn quiet() -> (Rules, EconomyState, LandCounts) {
        let (mut rules, mut state, land) = march();
        // The place burns no fuel and wants no fish, so wood and fish are worth nothing to it: the forest is tried, and no move
        // passes.
        rules
            .pop_types
            .iter_mut()
            .find(|p| p.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 0;
        let place = &mut state.places[0];
        place.pops[0].workers = 20;
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.price_drift_carries = vec![0; rules.goods.len()];
        for pop in &mut place.pops {
            pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            pop.consumption.ticking_carries = vec![0; rules.goods.len()];
            pop.employment.truncate(1);
            pop.consumption.necessities_pm = 1000;
            pop.consumption.niceties_pm = 1000;
            pop.consumption.luxuries_pm = 1000;
        }
        (rules, state, land)
    }

    #[test]
    fn where_no_move_passes_two_years_are_day_for_day_the_build_without_switching() {
        let (rules, state, land) = quiet();
        // Wood is tried, and no move passes.
        let tried = look(
            &rules,
            &state.places[0],
            &land,
            0,
            3,
            &vec![1000; rules.goods.len()],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(!tried.moves.is_empty() && tried.answer.is_none());
        let mut plain = state.clone();
        let mut switching = state;
        let lands = BTreeMap::from([("fixture".to_owned(), land)]);
        let mut date = Date {
            year: YEAR,
            month: 1,
            day: 1,
            hour: 12,
        };
        for _ in 0..=(2 * 365) {
            let (a, b) = (
                crate::run_daily_economy(&rules, &mut plain, date).unwrap(),
                crate::run_daily_economy_with_land(
                    &rules,
                    &mut switching,
                    date,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &lands,
                )
                .unwrap(),
            );
            assert_eq!(a, b, "{date:?}");
            assert_eq!(plain, switching, "{date:?}");
            date = next_day(date);
        }
    }

    /// The march fixture made ready for the whole daily step (valid dimensions), with the place's own prices.
    fn daily_march(wood_price: u64) -> (Rules, EconomyState, LandCounts) {
        let (rules, mut state, land) = march();
        let place = &mut state.places[0];
        place.pops[0].workers = 110;
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.prices_milli[good_at(&rules, "hardwood")] = wood_price;
        place.price_drift_carries = vec![0; rules.goods.len()];
        for pop in &mut place.pops {
            pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            pop.consumption.ticking_carries = vec![0; rules.goods.len()];
            pop.employment.truncate(1);
            pop.consumption.necessities_pm = 1000;
            pop.consumption.niceties_pm = 1000;
            pop.consumption.luxuries_pm = 1000;
        }
        (rules, state, land)
    }

    #[test]
    fn the_daily_step_looks_at_the_prices_the_day_began_with_though_the_price_step_would_reverse_the_move()
     {
        // Oats are 15,000 and wood 5,000 as the day begins: the move to wood passes. The day's price step moves them (wood to 6,000
        // and oats to 28,500), and at those prices the move fails. The level moves: the look was made at the opening prices.
        let (mut rules, mut state, land) = daily_march(5_000);
        let wool_index = good_at(&rules, "wool");
        rules.goods[wool_index].stacked_outside = true;
        state.places[0].pops[0].stock[wool_index] = 1_000_000;
        // Keep this price-order witness on a growing candidate.
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[2]
            .grows = true;
        let (oats, wood) = (good_at(&rules, "oats"), good_at(&rules, "hardwood"));
        state.places[0].prices_milli[oats] = 15_000;
        let opening = state.places[0].prices_milli.clone();
        let mut moved_prices = state.clone();
        crate::update_daily_prices(&rules, &mut moved_prices).unwrap();
        let closing = moved_prices.places[0].prices_milli.clone();
        assert_eq!((closing[wood], closing[oats]), (6_000, 28_500));
        let asks = |prices: &[u64]| {
            look_in_year(
                &rules,
                &state.places[0],
                &land,
                0,
                3,
                Some(YEAR),
                prices,
                &BTreeMap::new(),
            )
            .unwrap()
            .answer
            .is_some()
        };
        assert!(asks(&opening), "the move passes at the opening prices");
        assert!(!asks(&closing), "and fails at the moved ones");
        crate::run_daily_economy_with_land(
            &rules,
            &mut state,
            on(3, YEAR),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::from([("fixture".to_owned(), land.clone())]),
        )
        .unwrap();
        assert_eq!(methods(&state, 0), vec![2]);
        assert_eq!(
            state.places[0].prices_milli[wood], 6_000,
            "the day's prices moved"
        );
    }

    #[test]
    fn the_look_reads_the_prices_the_day_began_with_and_not_the_place_s_moved_ones() {
        let (rules, state, land) = march();
        let goods = rules.goods.len();
        let wood = good_at(&rules, "hardwood");
        // The place's own prices make wood worthless; the day began with wood worth a lot.
        let mut case = state.clone();
        case.places[0].prices_milli = vec![1; goods];
        case.places[0].prices_milli[wood] = 0;
        let mut day = vec![vec![1_u64; goods]];
        day[0][wood] = 1000;
        let mut by_day_prices = case.clone();
        run_monthly_production_switching(
            &rules,
            &mut by_day_prices,
            on(3, YEAR),
            &BTreeMap::new(),
            &BTreeMap::from([("fixture".to_owned(), land.clone())]),
            &day,
        )
        .unwrap();
        assert_eq!(methods(&by_day_prices, 0), vec![2]);
        day[0][wood] = 0;
        let mut worthless = case;
        case_step(&rules, &mut worthless, &land, &day);
        assert_eq!(methods(&worthless, 0), vec![0]);
    }

    fn case_step(rules: &Rules, state: &mut EconomyState, land: &LandCounts, day: &[Vec<u64>]) {
        run_monthly_production_switching(
            rules,
            state,
            on(3, YEAR),
            &BTreeMap::new(),
            &BTreeMap::from([("fixture".to_owned(), land.clone())]),
            day,
        )
        .unwrap();
    }

    /// Five years of one place with two farms of ten levels each (the second a little poorer in cattle), herds that breed, forest,
    /// bog and sea, where the prices of fish, oats, wood and peat each take their turn at being the best: the moves made, as
    /// (year, month, building, level, from, to).
    fn replay() -> Vec<(u16, u8, usize, u32, u32, u32)> {
        let (mut rules, mut state, mut land, _) = culling(80);
        let shipped = level_rules();
        rules.herd_species = shipped.herd_species;
        let cattle = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        rules.herd_species[cattle].yields.push(HerdYield {
            good: "mutton".into(),
            units: 5,
        });
        for good in ["oats", "fish"] {
            let i = good_at(&rules, good);
            rules.goods[i].hunger_centi = 36_500;
        }
        rules.diet_profiles.insert(
            "peasant".into(),
            BTreeMap::from([("oats".into(), 500), ("fish".into(), 500)]),
        );
        rules
            .pop_types
            .iter_mut()
            .find(|p| p.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 1000;
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 1000)]);
        def.methods[1].annual_output = BTreeMap::from([("fish".into(), 1000)]);
        def.methods[2].annual_output = BTreeMap::from([("hardwood".into(), 1000)]);
        def.methods[3].annual_output = BTreeMap::from([("peat".into(), 1000)]);
        let mut second = state.places[0].buildings[0].clone();
        second.herds[cattle] = 60;
        state.places[0].buildings.push(second);
        land.land_cells.insert("forest".into(), 12);
        land.land_cells.insert("bog".into(), 6);
        let (oats, fish, wood, peat) = (
            good_at(&rules, "oats"),
            good_at(&rules, "fish"),
            good_at(&rules, "hardwood"),
            good_at(&rules, "peat"),
        );
        let mut moves = Vec::new();
        for year in YEAR..YEAR + 5 {
            for month in 1..=12_u8 {
                // Each year one good is dear and the rest cheap.
                let dear = [fish, wood, peat, oats, fish][usize::from(year - YEAR)];
                let mut day = vec![vec![100_u64; rules.goods.len()]];
                day[0][dear] = 400;
                let before = [methods(&state, 0), methods(&state, 1)];
                let ledger = run_monthly_production_switching(
                    &rules,
                    &mut state,
                    on(month, year),
                    &BTreeMap::new(),
                    &BTreeMap::from([("fixture".to_owned(), land.clone())]),
                    &day,
                )
                .unwrap();
                let place = &ledger.places[0];
                for kind in 0..place.herds_before.len() {
                    assert_eq!(
                        place.herds_before[kind]
                            + place.herd_births[kind]
                            + place.herd_arrivals[kind],
                        place.herds_after[kind]
                            + place.herd_deaths[kind]
                            + place.herd_slaughter[kind],
                        "{year}-{month} kind {kind}"
                    );
                }
                let after = [methods(&state, 0), methods(&state, 1)];
                for building in 0..2 {
                    for (level, (was, now)) in
                        before[building].iter().zip(&after[building]).enumerate()
                    {
                        if was != now {
                            moves.push((
                                year,
                                month,
                                building,
                                u32::try_from(level).unwrap(),
                                *was,
                                *now,
                            ));
                        }
                    }
                }
                validate_level_records(&rules, &BTreeMap::new(), &state).unwrap();
            }
        }
        // No level moves twice in a calendar month.
        let mut seen = std::collections::BTreeSet::new();
        for (year, month, building, level, _, _) in &moves {
            assert!(
                seen.insert((*year, *month, *building, *level)),
                "building {building} level {level} moved twice in {year}-{month}"
            );
        }
        moves
    }

    #[test]
    fn a_five_year_replay_of_two_buildings_makes_the_literal_moves() {
        // The moves are (year, month, building, level, from, to) with methods in the farm's order: mixed farming, sea, wood,
        // turf. Each year one good is dear: fish in 768, wood in 769, peat in 770, oats in 771 and fish again in 772. The earlier
        // building looks first, so where the forest's twelve cells (three wood levels) or the turf's six run short in 769 and
        // 770 it is the later building that waits. Typed staffing and herd caps change the literal moves from the earlier model.
        // No level moves twice in a month and the herds add up for the pass in every
        // month (both checked in the replay), and every month's state is a loadable one.
        assert_eq!(
            replay(),
            vec![
                (768, 1, 0, 0, 0, 1),
                (768, 1, 1, 0, 0, 1),
                (768, 2, 0, 1, 0, 1),
                (768, 2, 1, 1, 0, 1),
                (768, 3, 0, 2, 0, 1),
                (768, 3, 1, 2, 0, 1),
                (768, 4, 0, 3, 0, 1),
                (768, 4, 1, 3, 0, 1),
                (768, 5, 0, 4, 0, 1),
                (768, 5, 1, 4, 0, 1),
                (769, 1, 0, 0, 1, 2),
                (769, 1, 1, 0, 1, 2),
                (769, 2, 0, 1, 1, 2),
                (770, 1, 0, 0, 2, 3),
                (770, 1, 1, 0, 2, 3),
                (770, 2, 0, 1, 2, 3),
                (772, 1, 0, 0, 3, 1),
                (772, 1, 1, 0, 3, 1),
                (772, 2, 0, 1, 3, 1),
            ]
        );
    }

    #[test]
    fn a_save_with_a_year_of_last_switch_after_the_game_is_refused_naming_the_level() {
        let (_, mut state, _) = march();
        state.places[0].buildings[0].level_records[0].last_switch = Some((YEAR, 3));
        crate::validate_switch_years(&state, YEAR, 3).unwrap();
        crate::validate_switch_years(&state, YEAR, 4).unwrap();
        for stamp in [(YEAR, 0), (YEAR, 13), (YEAR, 4)] {
            state.places[0].buildings[0].level_records[0].last_switch = Some(stamp);
            let error = crate::validate_switch_years(&state, YEAR, 3).unwrap_err();
            assert!(
                error.contains("fixture") && error.contains("farm") && error.contains("level 0")
            );
        }
        state.places[0].buildings[0].level_records[0].last_switch = Some((YEAR - 1, 12));
        crate::validate_switch_years(&state, YEAR, 1).unwrap();
        state.places[0].buildings[0].level_records[0].last_switch = Some((YEAR, 3));
        let error = crate::validate_switch_years(&state, YEAR - 1, 3).unwrap_err();
        assert!(
            error.contains("fixture") && error.contains("farm") && error.contains("level 0"),
            "{error}"
        );
        let bytes = state.to_bytes().unwrap();
        assert_eq!(EconomyState::from_bytes(&bytes).unwrap(), state);
    }
}
