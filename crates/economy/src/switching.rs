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
    BuildingState, BuildingType, DataError, LandCounts, LevelRecord, Method, PlaceState, Rules,
};
use super::{
    building_coverage_pm, choose_terrain, herd_supplies, mean_fertility, method_suitability_pm,
    missing_data, projected_pop_order_units,
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
    let positions = u64::from(building.levels) * u64::from(def.workers_per_level);
    let workers = building
        .employment
        .iter()
        .try_fold(0u64, |sum, e| sum.checked_add(e.workers))
        .ok_or_else(|| missing_data("employment".into(), "worker count overflow"))?
        .min(positions);
    let coverage = u64::from(building_coverage_pm(def, &place.pops, building)?);
    let efficiency = 1000 - u64::from(def.penalty_pm) * (1000 - coverage) / 1000;
    Ok((workers, positions, efficiency))
}
fn amount(n: &[u128], d: &[u128]) -> Fraction {
    Fraction::new(super::bigint::product(n), super::bigint::product(d))
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
) -> Result<Vec<Fraction>, DataError> {
    let mut values = vec![Fraction::integer(0); rules.goods.len()];
    let (workers, positions, efficiency) = staffing(place, building, def)?;
    if positions == 0 {
        return Ok(values);
    }
    for (good, annual) in &method.annual_output {
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
                u128::from(workers),
                u128::from(efficiency),
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
        );
    }
    Ok(values)
}
fn herd_output(
    rules: &Rules,
    place: &PlaceState,
    building: &BuildingState,
    def: &BuildingType,
    heads: &[u64],
) -> Result<Vec<Fraction>, DataError> {
    let mut result = vec![Fraction::integer(0); rules.goods.len()];
    let (_, _, coverage) = staffing(place, building, def)?;
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
                    u128::from(coverage),
                ],
                &[1000, 1000],
            );
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
fn wanted(
    rules: &Rules,
    place: &PlaceState,
    jobs: &BTreeMap<String, u64>,
    excluded: usize,
    level: u32,
    month: u8,
    quality: u16,
) -> Result<Wanted, DataError> {
    let zero = Fraction::integer(0);
    let mut goods = vec![zero.clone(); rules.goods.len()];
    let mut shortages = vec![zero.clone(); rules.goods.len()];
    let mut heat = zero.clone();
    for pop in &place.pops {
        let kind = rules
            .pop_types
            .iter()
            .find(|p| p.id == pop.kind)
            .ok_or_else(|| missing_data(format!("pop_types.{}", pop.kind), "missing"))?;
        heat = heat.add(&amount(
            &[
                u128::from(pop.workers) + u128::from(pop.dependents),
                u128::from(kind.fuel_need_milli_day),
                365,
            ],
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
    let mut produced = vec![zero.clone(); rules.goods.len()];
    for (index, building) in place.buildings.iter().enumerate() {
        let def = definition(rules, building)?;
        for record in &building.level_records {
            if index == excluded && record.number == level {
                continue;
            }
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
            )?;
            for (total, item) in produced.iter_mut().zip(made) {
                *total = total.add(&item);
            }
        }
        if index != excluded {
            let herd = herd_output(rules, place, building, def, &building.herds)?;
            for (total, item) in produced.iter_mut().zip(herd) {
                *total = total.add(&item);
            }
        }
    }
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
    Ok(Wanted {
        goods,
        jobs: shortages,
        heat,
    })
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
#[expect(
    clippy::option_option,
    reason = "Outer none means no room; inner none means a valid sea method."
)]
fn target_ground(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    building_index: usize,
    level: &LevelRecord,
    method_index: usize,
) -> Result<Option<Option<String>>, DataError> {
    let def = definition(rules, &place.buildings[building_index])?;
    let method = &def.methods[method_index];
    if method.resource == "sea" {
        return Ok((land.sea > 0).then_some(None));
    }
    if method.resource == "forest" || method.resource == "bog" {
        let used: u64 = place
            .buildings
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let d = definition(rules, b).expect("validated building");
                b.level_records
                    .iter()
                    .filter(|r| !(i == building_index && r.number == level.number))
                    .filter(|r| d.methods[r.method as usize].id == method.id)
                    .count() as u64
            })
            .sum();
        let limit = u64::from(land.land_cells.get(&method.resource).copied().unwrap_or(0))
            / u64::from(method.cells_per_level.unwrap_or(1));
        return Ok((used < limit).then_some(Some(method.resource.clone())));
    }
    let mut remaining = land.clone();
    for (i, b) in place.buildings.iter().enumerate() {
        let d = definition(rules, b)?;
        for r in &b.level_records {
            if i == building_index && r.number == level.number {
                continue;
            }
            if d.methods[r.method as usize].resource == "usable_land"
                && let Some(ground) = &r.ground
            {
                let available = remaining.fertility_pm.entry(ground.clone()).or_default();
                *available = available.saturating_sub(u64::from(d.land_per_level) * 1000);
            }
        }
    }
    Ok(choose_terrain(
        rules,
        method,
        &remaining,
        u64::from(def.land_per_level) * 1000,
    )
    .map(Some))
}
fn switching_heads(
    rules: &Rules,
    building: &BuildingState,
    def: &BuildingType,
    level: &LevelRecord,
    target: &Method,
) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let mut keeping = building
        .level_records
        .iter()
        .filter(|r| def.methods[r.method as usize].resource != "sea")
        .count() as u64;
    if def.methods[level.method as usize].resource != "sea" {
        keeping -= 1;
    }
    if target.resource != "sea" {
        keeping += 1;
    }
    let mut next = Vec::new();
    let mut gone = Vec::new();
    let mut founders = Vec::new();
    for (i, species) in rules.herd_species.iter().enumerate() {
        let current = building.herds.get(i).copied().unwrap_or(0);
        let maximum =
            keeping.saturating_mul(def.herds.get(&species.animal).map_or(0, |c| c.maximum));
        let founder = if current == 0 && target.resource != "sea" {
            keeping.saturating_mul(2).min(maximum)
        } else {
            0
        };
        next.push(if current == 0 {
            founder
        } else {
            current.min(maximum)
        });
        gone.push(current.saturating_sub(maximum));
        founders.push(founder);
    }
    (next, gone, founders)
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
/// Calculates one building's best method switch without changing the place.
///
/// # Errors
/// Returns a data error for invalid inputs or missing rule definitions.
#[expect(
    clippy::too_many_lines,
    reason = "Each alternative follows the score and loss terms together in the contract's order."
)]
pub fn look(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    building_index: usize,
    month: u8,
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
    let quality = mean_fertility(land)?;
    let mut moves: Vec<MoveLook> = Vec::new();
    let mut answer: Option<usize> = None;
    let mut ordered_levels: Vec<&LevelRecord> = building.level_records.iter().collect();
    ordered_levels.sort_by_key(|level| level.number);
    for (target_index, target) in def.methods.iter().enumerate() {
        for level in &ordered_levels {
            if target_index == level.method as usize {
                continue;
            }
            let Some(ground) =
                target_ground(rules, place, land, building_index, level, target_index)?
            else {
                continue;
            };
            let want = wanted(
                rules,
                place,
                jobs,
                building_index,
                level.number,
                month,
                quality,
            )?;
            let old_method = &def.methods[level.method as usize];
            let old = output(
                rules,
                place,
                building,
                def,
                old_method,
                level.ground.as_deref(),
                level.paydays,
                month,
                quality,
                true,
            )?;
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
            )?;
            let slow = output(
                rules,
                place,
                building,
                def,
                target,
                ground.as_deref(),
                0,
                month,
                quality,
                false,
            )?;
            let old_value = value(rules, &old, &want, prices);
            let new_value = value(rules, &new, &want, prices);
            let (heads, gone, founders) = switching_heads(rules, building, def, level, target);
            let old_herd = value(
                rules,
                &herd_output(rules, place, building, def, &building.herds)?,
                &want,
                prices,
            );
            let new_herd = value(
                rules,
                &herd_output(rules, place, building, def, &heads)?,
                &want,
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
            let slow_start = new_value.saturating_sub(&value(rules, &slow, &want, prices));
            let herd_cost = herd_loss(rules, &gone, &founders, prices)?;
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

#[cfg(test)]
mod fraction_tests {
    use super::*;
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
        farm.level_records = (0..10)
            .map(|n| crate::tests::level(n, 0, Some("pasture"), 13))
            .collect();
        let mut prices = vec![1; rules.goods.len()];
        prices[crate::tests::good_at(&rules, "cattle")] = 240;
        for (heads, expected) in [(100, 0), (118, 900)] {
            farm.herds[cattle] = heads;
            let (_, gone, founders) =
                switching_heads(&rules, farm, def, &farm.level_records[0], &def.methods[1]);
            assert_eq!(
                herd_loss(&rules, &gone, &founders, &prices).unwrap(),
                Fraction::integer(expected)
            );
        }
        prices[crate::tests::good_at(&rules, "cattle")] = 100;
        let (_, gone, founders) =
            switching_heads(&rules, farm, def, &farm.level_records[0], &def.methods[1]);
        assert_eq!(
            herd_loss(&rules, &gone, &founders, &prices).unwrap(),
            Fraction::integer(0)
        );
        prices[crate::tests::good_at(&rules, "cattle")] = 240;
        farm.levels = 3;
        farm.level_records = (0..3)
            .map(|n| crate::tests::level(n, 1, None, 13))
            .collect();
        farm.level_records[0].method = 0;
        farm.level_records[0].ground = Some("pasture".into());
        farm.level_records[1].method = 0;
        farm.level_records[1].ground = Some("pasture".into());
        farm.herds.fill(0);
        let (heads, _, founders) =
            switching_heads(&rules, farm, def, &farm.level_records[2], &def.methods[0]);
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
        let (_, _, founders) =
            switching_heads(&rules, farm, def, &farm.level_records[2], &def.methods[0]);
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
