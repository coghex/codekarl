//! Whole-unit allocation for wage claims.

/// Allocate units of one good among claims in pop order, without overpaying.
///
/// Claims and the unit price use the same value units. A zero price cannot
/// settle a claim. Capped shares are redistributed until no more units fit.
#[must_use]
pub fn allocate_claims(claims: &[u64], price: u64, units: u64) -> Vec<u64> {
    let mut result = vec![0_u64; claims.len()];
    if price == 0 {
        return result;
    }
    let mut remaining = units;
    while remaining > 0 {
        let eligible: Vec<(usize, u64, u64)> = claims
            .iter()
            .enumerate()
            .filter_map(|(i, &claim)| {
                let balance = claim - result[i] * price;
                let room = balance / price;
                (room > 0).then_some((i, balance, room))
            })
            .collect();
        let total: u128 = eligible
            .iter()
            .map(|(_, balance, _)| u128::from(*balance))
            .sum();
        if total == 0 {
            break;
        }
        let mut shares: Vec<(usize, u64, u128, u64)> = eligible
            .iter()
            .map(|&(i, balance, room)| {
                let numerator = u128::from(remaining) * u128::from(balance);
                (
                    i,
                    u64::try_from(numerator / total)
                        .unwrap_or(u64::MAX)
                        .min(room),
                    numerator % total,
                    room,
                )
            })
            .collect();
        let assigned: u64 = shares.iter().map(|(_, share, _, _)| *share).sum();
        let mut left = remaining - assigned;
        shares.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        for (_, share, _, room) in &mut shares {
            if left == 0 {
                break;
            }
            if *share < *room {
                *share += 1;
                left -= 1;
            }
        }
        let allocated = remaining - left;
        if allocated == 0 {
            break;
        }
        for (i, share, _, _) in shares {
            result[i] += share;
        }
        remaining = left;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::allocate_claims;

    #[test]
    fn whole_units_never_overpay_and_capped_shares_are_redistributed() {
        assert_eq!(allocate_claims(&[5], 10, 1), [0]);
        assert_eq!(allocate_claims(&[1, 9], 10, 1), [0, 0]);
        assert_eq!(allocate_claims(&[4, 16], 5, 3), [0, 3]);
        assert_eq!(allocate_claims(&[5, 100], 5, 15), [1, 14]);
    }

    #[test]
    fn largest_remainders_break_ties_by_pop_order() {
        assert_eq!(allocate_claims(&[10, 10, 10], 1, 2), [1, 1, 0]);
        assert_eq!(allocate_claims(&[10, 10], 0, 2), [0, 0]);
    }
}
/// Minimum monthly wage in thousandths, with one final rounding down.
/// Prices and necessities are in goods order; `None` indicates malformed input
/// or an unrepresentable result.
#[must_use]
pub fn minimum_wage(
    prices: &[u64],
    necessities: &[u32],
    workers: u64,
    dependents: u64,
    dependent_pm: u16,
    minimum_pm: u16,
    fulfilment_pm: u16,
) -> Option<u64> {
    if prices.len() != necessities.len() {
        return None;
    }
    let cheapest = prices
        .iter()
        .copied()
        .zip(necessities.iter().copied())
        .filter(|(_, n)| *n > 0)
        .min_by(|(a, an), (b, bn)| {
            (u128::from(*a) * u128::from(*bn)).cmp(&(u128::from(*b) * u128::from(*an)))
        })?;
    let (house_num, house_den) = if workers == 0 {
        (u128::from(1000 + u32::from(dependent_pm)), 1000)
    } else {
        (
            u128::from(workers) + u128::from(dependents),
            u128::from(workers),
        )
    };
    let mut numerator = super::bigint::Big::new(u128::from(cheapest.0));
    for factor in [
        3000,
        house_num,
        u128::from(minimum_pm),
        u128::from(fulfilment_pm.min(1000)),
    ] {
        numerator = numerator.mul(&super::bigint::Big::new(factor));
    }
    let denominator = super::bigint::Big::new(u128::from(cheapest.1))
        .mul(&super::bigint::Big::new(house_den))
        .mul(&super::bigint::Big::new(1_000_000));
    u64::try_from(numerator.divmod(&denominator).0.to_u128()?).ok()
}

/// Minimum for a destination population type using the source household and hunger.
///
/// Returns `None` for missing type, invalid prices, absent necessities, or overflow.
#[must_use]
pub fn pop_minimum_wage(
    rules: &super::Rules,
    prices: &[u64],
    source: &super::PopState,
    destination_kind: &str,
) -> Option<u64> {
    let kind = rules
        .pop_types
        .iter()
        .find(|kind| kind.id == destination_kind)?;
    let necessities = rules
        .goods
        .iter()
        .map(|good| good.necessities_centi)
        .collect::<Vec<_>>();
    minimum_wage(
        prices,
        &necessities,
        source.workers,
        source.dependents,
        kind.dependent_pm,
        kind.min_wage_pm,
        source.consumption.necessities_pm,
    )
}

#[cfg(test)]
mod minimum_tests {
    use super::minimum_wage;
    #[test]
    fn household_hunger_and_destination_type() {
        // 200 milli per hundredth of a person-day makes 6000 per month.
        assert_eq!(
            minimum_wage(&[200], &[100], 40, 16, 400, 500, 1000),
            Some(4200)
        );
        assert_eq!(
            minimum_wage(&[200], &[100], 40, 16, 400, 500, 500),
            Some(2100)
        );
        assert_eq!(
            minimum_wage(&[200], &[100], 0, 7, 400, 500, 1000),
            Some(4200)
        );
        assert_eq!(
            minimum_wage(&[200], &[100], 40, 16, 400, 250, 1000),
            Some(2100)
        );
    }
    #[test]
    fn cheapest_necessity_and_final_rounding() {
        assert_eq!(
            minimum_wage(&[201, 100], &[100, 100], 1, 0, 0, 1000, 1000),
            Some(3000)
        );
        assert_eq!(minimum_wage(&[1], &[7], 1, 0, 0, 1, 1), Some(0));
        assert_eq!(minimum_wage(&[1], &[0], 1, 0, 0, 1000, 1000), None);
    }
}
/// Sum job earnings for a calendar month, rounding down once after all jobs.
/// Each pair contains the committed monthly offer and the staffed person-days.
#[must_use]
pub fn monthly_earnings(jobs: &[(u64, u64)], month_days: u64) -> Option<u64> {
    use super::bigint::Big;
    if month_days == 0 {
        return None;
    }
    let numerator = jobs.iter().fold(Big::new(0), |sum, &(offer, days)| {
        sum.add(&Big::new(u128::from(offer) * u128::from(days)))
    });
    u64::try_from(
        numerator
            .divmod(&Big::new(u128::from(month_days)))
            .0
            .to_u128()?,
    )
    .ok()
}

#[cfg(test)]
mod earnings_tests {
    use super::monthly_earnings;

    #[test]
    fn calendar_month_and_absence_proration() {
        assert_eq!(monthly_earnings(&[(30, 20)], 30), Some(20));
        assert_eq!(monthly_earnings(&[(31, 31)], 31), Some(31));
        assert_eq!(monthly_earnings(&[(31, 28)], 28), Some(31));
        assert_eq!(monthly_earnings(&[(1, 15), (1, 15)], 30), Some(1));
        assert_eq!(monthly_earnings(&[(1, 1)], 0), None);
    }
}
