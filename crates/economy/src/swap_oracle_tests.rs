//! CK-391 (spec r6): the swap's size against an exhaustive search.
//!
//! `best_pair` finds rule 11's pair by halving and a bounded run of payments, and by arithmetic at a loss limit of nothing, never by
//! trying amounts. Reading it is not proof, so these tests set it beside the plain statement of rules 10, 11 and 13: every pair of
//! amounts, the most received and of those the fewest given, each tested whole against spare and want, worth, loss, capacity and the
//! room of each pop's own store as it stands. The cases are small and drawn from a fixed generator, so every run is the same; each
//! sets the prices, volumes, limits, holdings, room, capacity and loss limit at random.

use std::fmt::Write as _;

use super::swap::{Book, Exchange, best_pair, best_pair_capped};
use super::*;
use crate::swap_tests;

/// A small deterministic generator (xorshift64*), so the cases are the same on every run and every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[usize::try_from(self.below(items.len() as u64)).unwrap()]
    }
}

struct Case {
    rules: Rules,
    place: PlaceState,
    book: Book,
    give_good: usize,
    get_good: usize,
    capacity_left: u128,
    summary: String,
}

/// Sets a pop's store so that exactly `free_ml` of its volume is free, with `wool` (one millilitre a unit), which nobody wants.
fn fill_pop_store(rules: &Rules, place: &mut PlaceState, pop: usize, free_ml: u128) {
    let wool = swap_tests::good(rules, "wool");
    let people = u128::from(place.pops[pop].workers + place.pops[pop].dependents);
    let used = volume_used(rules, &place.pops[pop].stock, None).unwrap();
    if let Some(units) = (people * 720_000).checked_sub(used + free_ml) {
        place.pops[pop].stock[wool] = u64::try_from(units).unwrap();
    }
}

/// One case drawn from `rng`: two pops and a pair of goods that each pop wants from the other.
fn draw(rng: &mut Rng) -> Case {
    let mut rules = swap_tests::rules();
    let mut place = swap_tests::place(&rules, 2);
    let (give_id, get_id) = rng.pick(&[
        ("oats", "fish"),
        ("fish", "barley"),
        ("honey", "oats"),
        ("barley", "herbs"),
    ]);
    let (give_good, get_good) = (
        swap_tests::good(&rules, give_id),
        swap_tests::good(&rules, get_id),
    );
    let wool = swap_tests::good(&rules, "wool");
    rules.goods[wool].volume_ml = 1;
    rules.swap_loss_limit_pm = rng.pick(&[0, 1, 7, 50, 100, 100, 300, 1000]);
    for good in [give_good, get_good] {
        rules.goods[good].volume_ml = rng.pick(&[0_u64, 1, 2, 3, 5, 7, 11, 13]);
        rules.goods[good].pop_limit_milli = rng.pick(&[
            30,
            200,
            600,
            2000,
            1_000_000_000,
            1_000_000_000,
            1_000_000_000,
        ]);
        place.prices_milli[good] =
            rng.pick(&[7, 500, 1000, 1000, 1000, 2000, 3000, 5000, 9000, 100_000]);
    }
    for pop in 0..2 {
        place.pops[pop].workers = rng.pick(&[8, 20, 50, 100, 200]);
        place.pops[pop].dependents = 0;
    }
    let mut target = vec![vec![0_u128; rules.goods.len()]; 2];
    let mut holding = vec![vec![0_u128; rules.goods.len()]; 2];
    let mut summary = format!(
        "{give_id} for {get_id}, volumes {}/{}, limits {}/{}, prices {}/{}, loss {}",
        rules.goods[give_good].volume_ml,
        rules.goods[get_good].volume_ml,
        rules.goods[give_good].pop_limit_milli,
        rules.goods[get_good].pop_limit_milli,
        place.prices_milli[give_good],
        place.prices_milli[get_good],
        rules.swap_loss_limit_pm
    );
    // The starter gives `give_good` it has spare and wants `get_good`; the partner the reverse.
    for (pop, good, spare) in [
        (0, give_good, true),
        (1, get_good, true),
        (0, get_good, false),
        (1, give_good, false),
    ] {
        let amount = 1 + rng.below(40);
        let held = if spare {
            amount + rng.below(20)
        } else {
            rng.below(25)
        };
        place.pops[pop].stock[good] = held;
        holding[pop][good] = u128::from(held);
        target[pop][good] = if spare {
            u128::from(held - amount)
        } else {
            u128::from(held + amount)
        };
        write!(
            summary,
            "; pop {pop} {} {amount} of {id} (holds {held})",
            if spare { "spares" } else { "wants" },
            id = rules.goods[good].id
        )
        .unwrap();
    }
    for pop in 0..2 {
        let free = rng.pick(&[0_u128, 8, 30, 60, 150, 10_000_000, 10_000_000, 10_000_000]);
        fill_pop_store(&rules, &mut place, pop, free);
        write!(summary, "; pop {pop} volume free {free}").unwrap();
    }
    let capacity_left = rng.pick(&[10_u128, 40, 100, 400, 10_000_000, 10_000_000]);
    write!(summary, "; capacity {capacity_left}").unwrap();
    Case {
        rules,
        place,
        book: Book::from_parts(target, holding),
        give_good,
        get_good,
        capacity_left,
        summary,
    }
}

/// The room a pop's own store has for a good as it stands: the good's limit and the store's free volume, stated without the
/// exchange's own helper.
fn room(case: &Case, pop: usize, good: usize) -> u128 {
    let people = u128::from(case.place.pops[pop].workers + case.place.pops[pop].dependents);
    let stock = &case.place.pops[pop].stock;
    let limit = u128::from(case.rules.goods[good].pop_limit_milli) * people / 1000;
    let by_limit = limit.saturating_sub(u128::from(stock[good]));
    let volume = u128::from(case.rules.goods[good].volume_ml);
    let free = (people * 720_000).saturating_sub(volume_used(&case.rules, stock, None).unwrap());
    if free == 0 {
        return 0;
    }
    if volume == 0 {
        return by_limit;
    }
    by_limit.min(free / volume)
}

/// Rules 10, 11 and 13 stated plainly: every pair, the most received and of those the fewest given.
fn exhaustive_capped(case: &Case, [give_cap, get_cap]: [u128; 2]) -> Option<(u64, u64)> {
    let (rules, place, book) = (&case.rules, &case.place, &case.book);
    let (give_good, get_good) = (case.give_good, case.get_good);
    let most_get = book
        .position(0, get_good)
        .want
        .min(book.position(1, get_good).spare)
        .min(room(case, 0, get_good))
        .min(get_cap);
    let most_give = book
        .position(0, give_good)
        .spare
        .min(book.position(1, give_good).want)
        .min(room(case, 1, give_good))
        .min(give_cap);
    let give_price = u128::from(place.prices_milli[give_good]);
    let get_price = u128::from(place.prices_milli[get_good]);
    let limit = u128::from(rules.swap_loss_limit_pm);
    let (give_volume, get_volume) = (
        u128::from(rules.goods[give_good].volume_ml),
        u128::from(rules.goods[get_good].volume_ml),
    );
    for get_units in (1..=most_get).rev() {
        for give_units in 1..=most_give {
            let (given, got) = (give_units * give_price, get_units * get_price);
            if given < got || (given - got) * 1000 > limit * given {
                continue;
            }
            if give_units * give_volume + get_units * get_volume > case.capacity_left {
                continue;
            }
            return Some((
                u64::try_from(give_units).unwrap(),
                u64::try_from(get_units).unwrap(),
            ));
        }
    }
    None
}

/// Runs `count` cases from `seed` and returns how many had a swap; the first case that differs fails the test.
fn compare(seed: u64, count: u64) -> u64 {
    let mut rng = Rng(seed);
    let mut swaps = 0;
    for index in 0..count {
        let case = draw(&mut rng);
        let exchange = Exchange {
            rules: &case.rules,
            house: None,
        };
        let expected = exhaustive_capped(&case, [u128::MAX; 2]);
        let found = best_pair(
            &exchange,
            &case.place,
            &case.book,
            [0, 1],
            [case.give_good, case.get_good],
            case.capacity_left,
        )
        .unwrap();
        assert_eq!(
            found, expected,
            "seed {seed} case {index}: the search and the exhaustive one differ: {}",
            case.summary
        );
        swaps += u64::from(expected.is_some());
    }
    swaps
}

#[test]
fn the_size_of_a_swap_is_the_best_pair_found_by_trying_every_pair() {
    let mut swaps = 0;
    for seed in 1..=10_u64 {
        swaps += compare(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 500);
    }
    // The cases are drawn to be hard, but a good share must still end in a swap, or the comparison proves little.
    assert!(swaps > 600, "only {swaps} of 5,000 cases made a swap");
}

#[test]
fn share_limits_on_both_legs_match_exhaustive_pairs() {
    let mut rng = Rng(0x6384_9751);
    for index in 0..5_000 {
        let case = draw(&mut rng);
        let caps = [u128::from(rng.below(40)), u128::from(rng.below(40))];
        let exchange = Exchange {
            rules: &case.rules,
            house: None,
        };
        let found = best_pair_capped(
            &exchange,
            &case.place,
            &case.book,
            [0, 1],
            [case.give_good, case.get_good],
            case.capacity_left,
            [
                case.book.position(0, case.give_good).spare,
                case.book.position(1, case.get_good).spare,
                caps[0],
                caps[1],
            ],
        )
        .unwrap();
        assert_eq!(
            found,
            exhaustive_capped(&case, caps),
            "case {index}: {}, caps {caps:?}",
            case.summary
        );
    }
}

/// A case of two pops of 100 million people (room for any amount below) with the prices, volumes, loss limit, amounts and capacity
/// given: the starter has `spare` of the good it gives and wants `want` of the good it receives, and the partner the reverse.
fn explicit(
    [give_price, get_price]: [u64; 2],
    [give_volume, get_volume]: [u64; 2],
    loss_pm: u16,
    [spare, want]: [u64; 2],
    capacity_left: u128,
) -> Case {
    let mut rules = swap_tests::rules();
    let mut place = swap_tests::place(&rules, 2);
    let (give_good, get_good) = (
        swap_tests::good(&rules, "oats"),
        swap_tests::good(&rules, "fish"),
    );
    rules.swap_loss_limit_pm = loss_pm;
    rules.goods[give_good].volume_ml = give_volume;
    rules.goods[get_good].volume_ml = get_volume;
    place.prices_milli[give_good] = give_price;
    place.prices_milli[get_good] = get_price;
    for pop in 0..2 {
        place.pops[pop].workers = 100_000_000;
        place.pops[pop].dependents = 0;
    }
    let goods = rules.goods.len();
    let (mut target, mut holding) = (vec![vec![0_u128; goods]; 2], vec![vec![0_u128; goods]; 2]);
    // The starter gives what it has spare and wants what the partner has spare; the partner wants what the starter gives.
    holding[0][give_good] = u128::from(spare);
    holding[1][get_good] = u128::from(want);
    target[0][get_good] = u128::from(want);
    target[1][give_good] = u128::from(spare);
    for (pop, good, units) in [(0, give_good, spare), (1, get_good, want)] {
        place.pops[pop].stock[good] = units;
    }
    Case {
        rules,
        place,
        book: Book::from_parts(target, holding),
        give_good,
        get_good,
        capacity_left,
        summary: "explicit".to_owned(),
    }
}

fn size_of(case: &Case) -> Option<(u64, u64)> {
    let exchange = Exchange {
        rules: &case.rules,
        house: None,
    };
    best_pair(
        &exchange,
        &case.place,
        &case.book,
        [0, 1],
        [case.give_good, case.get_good],
        case.capacity_left,
    )
    .unwrap()
}

#[test]
fn a_full_store_takes_nothing_so_no_pair_is_found_even_for_a_good_of_no_volume() {
    // The starter gives oats (a millilitre a unit) for fish that take no room (0 ml): 20 spare, 40 wanted, prices 101 and 30.
    let open = explicit([101, 30], [1, 0], 100, [20, 40], 10_000_000);
    assert!(size_of(&open).is_some());
    assert_eq!(size_of(&open), exhaustive_capped(&open, [u128::MAX; 2]));
    // The same case with the starter's store exactly at its volume limit (rule 13): nothing moves, as the trying of every pair finds.
    let mut full = explicit([101, 30], [1, 0], 100, [20, 40], 10_000_000);
    let wool = swap_tests::good(&full.rules, "wool");
    full.rules.goods[wool].volume_ml = 1;
    fill_pop_store(&full.rules, &mut full.place, 0, 0);
    let people = u128::from(full.place.pops[0].workers + full.place.pops[0].dependents);
    assert_eq!(
        volume_used(&full.rules, &full.place.pops[0].stock, None).unwrap(),
        people * 720_000
    );
    assert_eq!(exhaustive_capped(&full, [u128::MAX; 2]), None);
    assert_eq!(size_of(&full), None);
}

#[test]
fn ten_million_each_way_is_found_at_once_and_a_smaller_capacity_halves_it() {
    // Two goods at one price, a millilitre a unit, each pop with 10,000,000 units spare of what the other wants 10,000,000 of, and
    // room for them. The starter's capacity is 22,500,000 ml: 10,000,000 for 10,000,000. With 15,000,000 ml: 7,500,000 for 7,500,000.
    let sizes = |capacity| {
        size_of(&explicit(
            [1000, 1000],
            [1, 1],
            100,
            [10_000_000, 10_000_000],
            capacity,
        ))
    };
    assert_eq!(sizes(22_500_000), Some((10_000_000, 10_000_000)));
    assert_eq!(sizes(15_000_000), Some((7_500_000, 7_500_000)));
}

#[test]
fn a_loss_limit_of_nothing_swaps_the_largest_multiple_of_the_smallest_pair_of_equal_worth() {
    // Prices 4,000,001 given and 3,000,001 received: the smallest pair of equal worth is 3,000,001 given for 4,000,001 received,
    // and two of it do not fit in 8,000,001.
    let capacity = u128::MAX;
    assert_eq!(
        size_of(&explicit(
            [4_000_001, 3_000_001],
            [0, 0],
            0,
            [100_000_000, 8_000_001],
            capacity
        )),
        Some((3_000_001, 4_000_001))
    );
    // Prices 100 and 70: the smallest pair is 7 for 10. Not even one fits in 9 units received; 25 take two.
    assert_eq!(
        size_of(&explicit([100, 70], [0, 0], 0, [100, 9], capacity)),
        None
    );
    assert_eq!(
        size_of(&explicit([100, 70], [0, 0], 0, [100, 25], capacity)),
        Some((14, 20))
    );
}

#[test]
fn a_few_dear_units_for_many_cheap_ones_pay_the_fewest_units_worth_the_receipt() {
    // The good given is worth 100,000 a unit and the good received 30, at a tenth. With 20,000 wanted: 6 for 20,000. With 19,000:
    // 6 for 19,000, a loss of 30,000 in 600,000. With 17,000, 6 units would lose 90,000 in 600,000, over a tenth; 5 units are worth
    // 16,666: 5 for 16,666.
    let sizes = |wanted| {
        size_of(&explicit(
            [100_000, 30],
            [0, 0],
            100,
            [1_000_000, wanted],
            u128::MAX,
        ))
    };
    assert_eq!(sizes(20_000), Some((6, 20_000)));
    assert_eq!(sizes(19_000), Some((6, 19_000)));
    assert_eq!(sizes(17_000), Some((5, 16_666)));
}
