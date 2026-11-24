//! What a place needs and orders for a year, and how the opening fills a building's levels by it.
//!
//! A place has two needs, food and fuel, and a yearly order for every good its people want. A level
//! is given a method by the first rule that gives one: an unmet need, then the empty orders, then
//! one more year of both, then the first food method with room (economy design §3.4).

use std::cmp::Ordering;

use super::{
    BuildingType, DataError, FOOD_NEED, FUEL_NEED, PopState, Rules, bigint::product, missing_data,
    projected_pop_order_units,
};

/// The yearly count of days the opening's needs and orders are worked for.
const YEAR_DAYS: u128 = 365;

const FOOD: usize = 0;
const FUEL: usize = 1;

/// One level's yearly output as the start works it out, by good index, in whole units.
pub(super) type LevelOutput = Vec<(usize, u128)>;

/// A method that has room for the next level, with what that level would make.
pub(super) struct Candidate {
    /// Index of the method in its building.
    pub(super) index: usize,
    /// The terrain a land level would use.
    pub(super) terrain: Option<String>,
    /// The level's yearly output.
    pub(super) output: LevelOutput,
}

/// A place's needs and orders, what its levels provide so far, and the amounts being worked to.
///
/// The counts carry from building to building: they belong to the place.
pub(super) struct StartDemand {
    /// One year's need: food in hundredths of a person-day, fuel in thousandths of a fuel unit.
    need: [u128; 2],
    /// One year's order for each good, in whole units.
    order: Vec<u128>,
    /// What the levels are working to meet: the needs and orders, raised by whole years.
    target_need: [u128; 2],
    target_order: Vec<u128>,
    /// What the levels placed so far provide.
    provided_need: [u128; 2],
    provided_goods: Vec<u128>,
}

/// The targets of a place before a raise, kept so a raise that ends in a cut can be undone.
pub(super) struct Targets {
    need: [u128; 2],
    order: Vec<u128>,
}

fn need_index(serves: &str) -> Option<usize> {
    match serves {
        FOOD_NEED => Some(FOOD),
        FUEL_NEED => Some(FUEL),
        _ => None,
    }
}

impl StartDemand {
    /// The needs and orders of `pops`, the people the start counts when it places levels.
    pub(super) fn new(rules: &Rules, pops: &[PopState]) -> Result<Self, DataError> {
        let mut need = [0_u128; 2];
        let mut order = vec![0_u128; rules.goods.len()];
        for pop in pops {
            let people = u128::from(pop.workers)
                .checked_add(u128::from(pop.dependents))
                .ok_or_else(|| missing_data("projection".to_owned(), "population sum overflow"))?;
            let fuel_milli_day = rules
                .pop_types
                .iter()
                .find(|item| item.id == pop.kind)
                .ok_or_else(|| {
                    missing_data(
                        format!("pop_types.{}", pop.kind),
                        "population type is missing",
                    )
                })?
                .fuel_need_milli_day;
            let year_days = YEAR_DAYS;
            let food = people
                .checked_mul(year_days * 100)
                .ok_or_else(|| missing_data("projection".to_owned(), "food need overflow"))?;
            let fuel = people
                .checked_mul(year_days * u128::from(fuel_milli_day))
                .ok_or_else(|| missing_data("projection".to_owned(), "fuel need overflow"))?;
            need[FOOD] = need[FOOD]
                .checked_add(food)
                .ok_or_else(|| missing_data("projection".to_owned(), "food need sum overflow"))?;
            need[FUEL] = need[FUEL]
                .checked_add(fuel)
                .ok_or_else(|| missing_data("projection".to_owned(), "fuel need sum overflow"))?;
            for (good_index, total) in order.iter_mut().enumerate() {
                // Fuel has no order here: the fuel need is what it is bought for, and the needs step fills it.
                if rules.goods[good_index].tags.iter().any(|tag| tag == "fuel") {
                    continue;
                }
                *total = total
                    .checked_add(projected_pop_order_units(rules, pop, good_index)?)
                    .ok_or_else(|| missing_data("projection".to_owned(), "order sum overflow"))?;
            }
        }
        Ok(Self {
            target_need: need,
            target_order: order.clone(),
            need,
            order,
            provided_need: [0; 2],
            provided_goods: vec![0; rules.goods.len()],
        })
    }

    /// One year's food need, in hundredths of a person-day.
    #[cfg(test)]
    pub(super) fn food_need(&self) -> u128 {
        self.need[FOOD]
    }

    /// One year's fuel need, in thousandths of a fuel unit.
    #[cfg(test)]
    pub(super) fn fuel_need(&self) -> u128 {
        self.need[FUEL]
    }

    /// What the levels placed so far provide of the food and fuel needs.
    #[cfg(test)]
    pub(super) fn provided_need(&self) -> (u128, u128) {
        (self.provided_need[FOOD], self.provided_need[FUEL])
    }

    /// The food and fuel needs the levels are working to.
    #[cfg(test)]
    pub(super) fn target_need(&self) -> (u128, u128) {
        (self.target_need[FOOD], self.target_need[FUEL])
    }

    /// One year's order for each good, in rules good order.
    #[cfg(test)]
    pub(super) fn orders(&self) -> &[u128] {
        &self.order
    }

    /// What the levels placed so far provide of each good, in rules good order.
    #[cfg(test)]
    pub(super) fn provided_goods(&self) -> &[u128] {
        &self.provided_goods
    }

    /// What a level with `output` provides towards a need.
    fn contribution(rules: &Rules, need: usize, output: &LevelOutput) -> u128 {
        output
            .iter()
            .map(|(good_index, units)| {
                let good = &rules.goods[*good_index];
                if need == FOOD {
                    units.saturating_mul(u128::from(good.hunger_centi))
                } else {
                    good.category_supplied("fuel", *units)
                }
            })
            .fold(0_u128, u128::saturating_add)
    }

    /// The needs not yet met, the least covered first and food when they are equally covered.
    ///
    /// The shares are compared exactly, as products of amounts that may each be near the limit of `u128`: `Big` does not overflow.
    fn unmet_needs(&self) -> Vec<usize> {
        let mut unmet = (0..2)
            .filter(|need| self.provided_need[*need] < self.target_need[*need])
            .collect::<Vec<_>>();
        if unmet.len() == 2 {
            let food = product(&[self.provided_need[FOOD], self.target_need[FUEL]]);
            let fuel = product(&[self.provided_need[FUEL], self.target_need[FOOD]]);
            if fuel.cmp(&food) == Ordering::Less {
                unmet.swap(0, 1);
            }
        }
        unmet
    }

    /// The value, at base prices, of the part of the empty orders that `output` would fill.
    fn empty_order_value(&self, rules: &Rules, output: &LevelOutput) -> u128 {
        output
            .iter()
            .map(|(good_index, units)| {
                let empty =
                    self.target_order[*good_index].saturating_sub(self.provided_goods[*good_index]);
                (*units).min(empty) * u128::from(rules.goods[*good_index].base_price)
            })
            .fold(0_u128, u128::saturating_add)
    }

    /// Steps one and two: the candidate for the next level by an unmet need, else by the empty
    /// orders. `levels` is the building's count of levels on each of its methods so far.
    pub(super) fn choose(
        &self,
        rules: &Rules,
        building: &BuildingType,
        candidates: &[Candidate],
        levels: &[u32],
    ) -> Option<usize> {
        for need in self.unmet_needs() {
            let chosen = candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    need_index(&building.methods[candidate.index].serves) == Some(need)
                        && Self::contribution(rules, need, &candidate.output) > 0
                })
                .min_by_key(|(_, candidate)| (levels[candidate.index], candidate.index));
            if let Some((position, _)) = chosen {
                return Some(position);
            }
        }
        let mut best: Option<(u128, usize)> = None;
        for (position, candidate) in candidates.iter().enumerate() {
            if need_index(&building.methods[candidate.index].serves) != Some(FOOD) {
                continue;
            }
            let value = self.empty_order_value(rules, &candidate.output);
            if value > 0 && best.is_none_or(|(most, _)| value > most) {
                best = Some((value, position));
            }
        }
        best.map(|(_, position)| position)
    }

    /// The first candidate in method order that serves food: where an empty place's levels go.
    pub(super) fn fallback(building: &BuildingType, candidates: &[Candidate]) -> Option<usize> {
        candidates.iter().position(|candidate| {
            need_index(&building.methods[candidate.index].serves) == Some(FOOD)
        })
    }

    /// Count the whole of a placed level's output as provided.
    pub(super) fn provide(&mut self, rules: &Rules, output: &LevelOutput) {
        for need in [FOOD, FUEL] {
            self.provided_need[need] =
                self.provided_need[need].saturating_add(Self::contribution(rules, need, output));
        }
        for (good_index, units) in output {
            self.provided_goods[*good_index] =
                self.provided_goods[*good_index].saturating_add(*units);
        }
    }

    /// Raise the needs and orders being worked to by one more year; the targets before are returned.
    pub(super) fn raise(&mut self) -> Targets {
        let before = Targets {
            need: self.target_need,
            order: self.target_order.clone(),
        };
        for need in [FOOD, FUEL] {
            self.target_need[need] = self.target_need[need].saturating_add(self.need[need]);
        }
        for (target, year) in self.target_order.iter_mut().zip(&self.order) {
            *target = target.saturating_add(*year);
        }
        before
    }

    /// Undo a raise that ended in a cut.
    pub(super) fn undo(&mut self, before: Targets) {
        self.target_need = before.need;
        self.target_order = before.order;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demand(provided: [u128; 2], target: [u128; 2]) -> StartDemand {
        StartDemand {
            need: target,
            order: Vec::new(),
            target_need: target,
            target_order: Vec::new(),
            provided_need: provided,
            provided_goods: Vec::new(),
        }
    }

    #[test]
    fn the_less_covered_need_goes_first() {
        // Food a quarter covered, fuel three quarters.
        assert_eq!(demand([25, 75], [100, 100]).unmet_needs(), [FOOD, FUEL]);
        assert_eq!(demand([75, 25], [100, 100]).unmet_needs(), [FUEL, FOOD]);
    }

    #[test]
    fn covered_shares_are_compared_exactly_not_by_rounding() {
        // 1 - 1/F against 1 - 1/U: the fuel gap is the larger by a trillionth of a trillionth,
        // which no float or per-mille rounding shows, so fuel is the less covered.
        let food = 1_000_000_000_000_u128;
        let fuel = 999_999_999_999_u128;
        let unmet = demand([food - 1, fuel - 1], [food, fuel]).unmet_needs();
        assert_eq!(unmet, [FUEL, FOOD]);
        // And with the amounts the other way round, food is.
        let unmet = demand([fuel - 1, food - 1], [fuel, food]).unmet_needs();
        assert_eq!(unmet, [FOOD, FUEL]);
    }

    #[test]
    fn equal_shares_give_food_first_and_a_met_need_is_not_listed() {
        assert_eq!(demand([1, 2], [2, 4]).unmet_needs(), [FOOD, FUEL]);
        assert_eq!(demand([0, 0], [10, 10]).unmet_needs(), [FOOD, FUEL]);
        assert_eq!(demand([10, 3], [10, 10]).unmet_needs(), [FUEL]);
        assert_eq!(
            demand([10, 10], [10, 10]).unmet_needs(),
            Vec::<usize>::new()
        );
        // A need of nothing is met.
        assert_eq!(demand([0, 0], [0, 0]).unmet_needs(), Vec::<usize>::new());
    }
}
