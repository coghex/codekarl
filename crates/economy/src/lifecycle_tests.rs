//! Life-cycle data contracts (CK-364): the shipped files load and state what they say, and each validation rule refuses the data it
//! names.

use crate::{
    DataError, LifeCycle, Rules,
    lifecycle::{Ages, Registry},
};
use std::{path::Path, sync::OnceLock};

fn asset(name: &str) -> String {
    // Read at run time, not with `include_str!`, so a crate built without the assets directory still compiles.
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/game/lifecycle/")
            .join(name),
    )
    .expect("shipped data")
}
fn life_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| asset("life-cycle.toml"))
}
fn names_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| asset("names.toml"))
}
fn shipped_rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .expect("shipped rules")
    })
}
fn parse(life: &str, names: &str) -> Result<LifeCycle, DataError> {
    let rules = shipped_rules();
    let pop_types: Vec<&str> = rules.pop_types.iter().map(|p| p.id.as_str()).collect();
    LifeCycle::parse(
        "life-cycle.toml",
        life,
        "names.toml",
        names,
        &Registry {
            peoples: &rules.peoples,
            pop_types: &pop_types,
            religions: &rules.religions,
        },
    )
}
fn edit(text: &str, from: &str, to: &str) -> String {
    assert!(
        text.contains(from),
        "the shipped text no longer contains {from:?}"
    );
    text.replacen(from, to, 1)
}
fn refused_life(from: &str, to: &str, entry: &str, problem: &str) {
    let error = parse(&edit(life_text(), from, to), names_text()).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert_eq!(error.file, "life-cycle.toml");
}
fn refused_names(from: &str, to: &str, entry: &str, problem: &str) {
    let error = parse(life_text(), &edit(names_text(), from, to)).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert_eq!(error.file, "names.toml");
}

#[test]
fn the_shipped_files_load_with_what_they_state() {
    let data = &shipped_rules().lifecycle;
    assert_eq!(&parse(life_text(), names_text()).expect("shipped"), data);
    let death = &data.death;
    assert_eq!(
        (
            death.infant_ppb,
            death.infant_years,
            death.base_ppb,
            death.old_ppb
        ),
        (200_000_000, 2, 7_000_000, 500_000)
    );
    assert_eq!(
        (death.old_from, death.old_doubling, death.certain_at),
        (20, 8, 105)
    );
    assert_eq!(
        (death.longevity_per_point, death.constitution_per_point),
        (-50_000, -40_000)
    );
    assert!(death.use_pop_curves);
    assert_eq!(
        (
            death.weight_low,
            death.weight_high,
            death.weight_factor,
            death.weight_step
        ),
        (800, 1250, 1200, 100)
    );
    assert_eq!(death.childbirth_ppb, 15_000_000);
    assert_eq!(data.majority.default, Ages { m: 17, f: 14 });
    assert_eq!(data.majority.of_group("english"), Ages { m: 15, f: 12 });
    assert_eq!(data.majority.of_group("brittonic"), Ages { m: 14, f: 12 });
    assert_eq!(data.majority.of_group("unlisted"), Ages { m: 17, f: 14 });
    assert_eq!(data.marriageable["christianity"], Ages { m: 14, f: 12 });
    let relations = &data.relations;
    assert_eq!(
        relations.mode_factor,
        [1_000_000_000, 500_000_000, 100_000_000]
    );
    assert_eq!(relations.wanted_children["noble"], 4);
    assert_eq!(relations.wanted_children["monk"], 2);
    assert_eq!(
        (relations.avoid_age, relations.avoid_necessities),
        (42, 600)
    );
    assert_eq!(relations.shift_at, 5000);
    let conception = &data.conception;
    assert_eq!(conception.monthly_ppb, 200_000_000);
    assert_eq!(conception.women[0], (15, 0));
    assert_eq!(conception.women.len(), 8);
    assert_eq!(conception.women[3], (35, 850_000_000));
    assert_eq!(conception.men.last(), Some(&(80, 50_000_000)));
    assert_eq!(conception.nursing_months, 12);
    assert_eq!(conception.church_abstinence_ppb, 850_000_000);
    assert_eq!(
        (
            conception.twins_ppb,
            conception.triplets_ppb,
            conception.twin_bearer_milli
        ),
        (12_500_000, 160_000, 3000)
    );
    assert_eq!(data.pools.len(), 4);
    assert_eq!(data.pools["irish"].men.len(), 48);
    assert_eq!(data.pools["irish"].women.len(), 18);
    assert_eq!(
        data.pools["irish"].patronym.as_ref().map(|p| p.m.as_str()),
        Some("mac")
    );
    assert_eq!(data.pools["english"].patronym, None);
    assert_eq!(data.pool_key("ulaid"), Some("irish"));
    assert_eq!(data.pool_key("northumbrian"), Some("english"));
    assert_eq!(data.pool_key("pictish"), Some("pictish"));
    assert_eq!(data.pool_key("nowhere"), None);
    // Every culture of the registry has a pool.
    for culture in &shipped_rules().peoples.cultures {
        assert!(data.pool_key(&culture.id).is_some(), "{}", culture.id);
    }
}

#[test]
fn a_probability_outside_zero_to_one_is_refused() {
    refused_life(
        "base = 0.007",
        "base = 1.5",
        "death.base",
        "must lie in 0..1",
    );
    refused_life(
        "childbirth = 0.015",
        "childbirth = -0.1",
        "death.childbirth",
        "",
    );
    refused_life(
        "monthly = 0.2 ",
        "monthly = 2.0 ",
        "conception.monthly",
        "must lie in 0..1",
    );
    refused_life(
        "twins = 0.0125",
        "twins = 1.25",
        "conception.twins",
        "must lie in 0..1",
    );
}

#[test]
fn age_curves_must_increase_and_not_be_negative() {
    refused_life(
        "women = [[15, 0.0], [20, 1.0]",
        "women = [[15, 0.0], [15, 1.0]",
        "conception.women",
        "strictly increasing",
    );
    refused_life(
        "women = [[15, 0.0], [20, 1.0]",
        "women = [[20, 0.0], [15, 1.0]",
        "conception.women",
        "strictly increasing",
    );
    refused_life(
        "men = [[15, 0.0], [18, 1.0]",
        "men = [[15, 0.0], [18, -1.0]",
        "conception.men",
        "must not be negative",
    );
    refused_life(
        "men = [[15, 0.0], [18, 1.0]",
        "men = [[15, 0.0], [18]",
        "conception.men",
        "a point is",
    );
}

#[test]
fn death_scales_must_be_positive_whole_numbers() {
    refused_life(
        "infant_years = 2",
        "infant_years = 0",
        "death.infant_years",
        "positive",
    );
    refused_life(
        "infant_years = 2",
        "infant_years = 1.5",
        "death.infant_years",
        "integer",
    );
    refused_life(
        "old_doubling = 8 ",
        "old_doubling = 0 ",
        "death.old_doubling",
        "positive",
    );
    refused_life(
        "certain_at = 105",
        "certain_at = 20",
        "death.old_from",
        "below certain_at",
    );
    refused_life(
        "weight_low = 0.80",
        "weight_low = 1.30",
        "death.weight_low",
        "below weight_high",
    );
}

#[test]
fn registry_keys_are_checked() {
    refused_life(
        "christianity = {",
        "druidry = {",
        "marriageable.druidry",
        "not a religion",
    );
    refused_life(
        "gaelic = {",
        "welsh = {",
        "majority.by_culture_group.welsh",
        "not a culture subgroup",
    );
    refused_life(
        "monk = 2,",
        "",
        "relations.wanted_children.monk",
        "missing value",
    );
    refused_life(
        "monk = 2,",
        "monk = 2, cleric = 1,",
        "relations.wanted_children.cleric",
        "unknown key",
    );
    refused_life(
        "trying = 1.0, ",
        "",
        "relations.mode_factor.trying",
        "missing value",
    );
}

#[test]
fn names_need_pools_for_every_culture_men_women_and_genitives() {
    refused_names(
        "ulaid = \"irish\"\n",
        "",
        "culture_pool.ulaid",
        "no name pool",
    );
    refused_names(
        "ulaid = \"irish\"",
        "ulaid = \"welsh\"",
        "culture_pool.ulaid",
        "is not a pool",
    );
    refused_names(
        "pictish = \"pictish\"",
        "atlantean = \"pictish\"",
        "culture_pool.atlantean",
        "not a culture",
    );
    refused_names(
        "women = [\"Der-Ilei\", \"Muirenn\", \"Eithne\"]",
        "women = []",
        "pool.pictish.women",
        "needs women",
    );
    refused_names(
        "men = [[\"Alpín\", \"Alpín\"], [\"Bridei\", \"Bridei\"],",
        "men = [[\"Alpín\", \"\"], [\"Bridei\", \"Bridei\"],",
        "pool.pictish.men",
        "no genitive",
    );
    refused_names(
        "men = [[\"Cadell\", \"Cadell\"],",
        "men = [[\"Cadell\"],",
        "pool.brittonic.men",
        "[name, genitive]",
    );
    // A pool without a patronym needs no genitive.
    parse(
        life_text(),
        &edit(names_text(), "[\"Offa\", \"Offaes\"]", "[\"Offa\", \"\"]"),
    )
    .expect("the English pool has no patronym");
}

// --- Death arithmetic (CK-364 §2) ---------------------------------------------------------------------------------------------

use crate::{
    bigint::Big,
    lifecycle::{DeathInputs, DeathRules},
};

fn death() -> &'static DeathRules {
    &shipped_rules().lifecycle.death
}
fn neutral(age: u32) -> DeathInputs {
    DeathInputs {
        age,
        constitution: 10_000,
        longevity: 10_000,
        pressure: 1_000_000_000,
        weight: 1000,
    }
}

#[test]
fn the_tables_and_the_annual_chance_match_independently_computed_values() {
    // Pinned from an independent computation of the specification's procedure (exact fractions, the same roundings).
    let rules = death();
    assert_eq!(rules.tables.infant.len(), 106);
    assert_eq!(rules.tables.exponent.len(), 86);
    assert_eq!(rules.tables.infant[1], 606_530_659, "E1 = e^(-1/2)");
    assert_eq!(
        rules.tables.exponent[1],
        Big::new(1_090_507_732),
        "R = 2^(1/8)"
    );
    // The old-age factor at age 0 reads the exponent table, not an age-indexed one.
    assert_eq!(rules.old_age_factor(0), Big::new(176_776_698));
    for (age, infant, old, annual) in [
        (0, 1_000_000_000, 176_776_698, 207_088_388),
        (1, 606_530_659, 192_776_356, 128_402_519),
        (2, 367_879_440, 210_224_107, 80_681_000),
        (19, 74_850, 917_004_043, 7_473_472),
        (20, 45_398, 1_000_000_000, 7_509_079),
        (40, 1, 5_656_854_153, 9_828_427),
        (60, 0, 31_999_999_036, 22_999_999),
        (80, 0, 181_019_328_297, 97_509_664),
        (100, 0, 1_023_999_944_003, 518_999_972),
    ] {
        assert_eq!(rules.infant_factor(age), infant, "E({age})");
        assert_eq!(rules.old_age_factor(age), Big::new(old), "G({age})");
        assert_eq!(rules.annual_ppb(&neutral(age)), annual, "p({age})");
    }
    // Death is certain from certain_at, and the chance is capped below it.
    assert_eq!(rules.annual_ppb(&neutral(105)), 1_000_000_000);
    assert_eq!(rules.annual_ppb(&neutral(200)), 1_000_000_000);
    assert_eq!(rules.annual_ppb(&neutral(104)), 731_077_302);
    let hungry = DeathInputs {
        pressure: 100_000_000_000,
        ..neutral(104)
    };
    assert_eq!(rules.annual_ppb(&hungry), 1_000_000_000, "capped at 1");
}

#[test]
fn the_chance_scales_with_the_pop_the_character_and_the_weight() {
    let rules = death();
    // K scales the base and old-age terms, not the infant term.
    let at = |age, pressure| {
        rules.annual_ppb(&DeathInputs {
            pressure,
            ..neutral(age)
        })
    };
    assert_eq!(at(40, 2_000_000_000), 19_656_854);
    assert_eq!(at(40, 500_000_000), 4_914_213);
    assert_eq!(at(0, 3_000_000_000), 221_265_165);
    // Constitution scales the base term x(1 - 0.04 x (constitution - 10)); longevity the old-age term x(1 - 0.05 x ...), both
    // floored at 0.3.
    let with = |age, constitution, longevity| {
        rules.annual_ppb(&DeathInputs {
            constitution,
            longevity,
            ..neutral(age)
        })
    };
    assert_eq!(with(40, 14_000, 10_000), 8_708_427);
    assert_eq!(with(40, 25_000, 10_000), 5_628_427);
    assert_eq!(with(40, 40_000, 10_000), 4_928_427, "the floor 0.3");
    assert_eq!(with(40, 25_000, 10_000) - with(40, 40_000, 10_000), 700_000);
    assert_eq!(with(80, 10_000, 3_000), 129_188_046);
    assert_eq!(with(80, 10_000, 30_000), 34_152_899);
    assert_eq!(with(80, 10_000, 40_000), 34_152_899, "the floor 0.3");
    assert_eq!(with(80, 10_000, 0), 142_764_496);
    // The weight multiplier scales the whole.
    let weighted = |age, weight| {
        rules.annual_ppb(&DeathInputs {
            weight,
            ..neutral(age)
        })
    };
    assert_eq!(weighted(40, 1200), 11_794_112);
    assert_eq!(weighted(40, 1300), 12_776_955);
    assert_eq!(weighted(0, 1200), 248_506_066);
    // The pop's multipliers apply only when the data says so.
    let without = DeathRules {
        use_pop_curves: false,
        ..rules.clone()
    };
    assert_eq!(
        without.annual_ppb(&DeathInputs {
            pressure: 7_000_000_000,
            ..neutral(40)
        }),
        without.annual_ppb(&neutral(40))
    );
}

#[test]
fn a_weight_extreme_multiplies_by_a_step_per_further_five_percent() {
    let rules = death();
    // A rolled weight of 70 kg: below 80% is under 56 kg and above 125% is over 87.5 kg.
    for (shown, expected) in [
        (70_000, 1000),
        (56_000, 1000),
        (55_999, 1200),
        (53_200, 1200),
        (52_500, 1300),
        (49_000, 1400),
        (87_500, 1000),
        (87_501, 1200),
        (91_000, 1300),
        (95_000, 1400),
    ] {
        assert_eq!(rules.weight_multiplier(shown, 70_000), expected, "{shown}");
    }
    assert_eq!(
        rules.weight_multiplier(10_000, 0),
        1000,
        "no rolled weight, no extreme"
    );
}

#[test]
fn the_monthly_chance_is_the_smallest_whose_twelve_rolls_reach_the_annual_one() {
    let exact = |annual: u32, monthly: u32| {
        let target =
            Big::new(u128::from(1_000_000_000 - annual)).mul(&Big::new(1_000_000_000).pow(11));
        let keeps = |m: u32| Big::new(u128::from(1_000_000_000 - m)).pow(12);
        assert_ne!(
            keeps(monthly).cmp(&target),
            std::cmp::Ordering::Greater,
            "{annual}: m is enough"
        );
        if monthly > 0 {
            assert_eq!(
                keeps(monthly - 1).cmp(&target),
                std::cmp::Ordering::Greater,
                "{annual}: m - 1 is not"
            );
        }
    };
    assert_eq!(DeathRules::monthly_ppb(200_000_000), 18_423_471);
    exact(200_000_000, 18_423_471);
    for (annual, monthly) in [
        (207_088_388, 19_151_201),
        (7_509_079, 627_921),
        (22_999_999, 1_937_174),
        (518_999_972, 59_167_976),
    ] {
        assert_eq!(DeathRules::monthly_ppb(annual), monthly);
        exact(annual, monthly);
    }
    assert_eq!(DeathRules::monthly_ppb(0), 0);
    assert_eq!(DeathRules::monthly_ppb(1_000_000_000), 1_000_000_000);
    // The monthly chance reproduces the annual one to within twelve ppb: 1 - (1 - m)^12 is at least the annual chance.
    for annual in [1_u32, 999, 123_456_789, 999_999_999] {
        let monthly = DeathRules::monthly_ppb(annual);
        exact(annual, monthly);
        assert!(monthly <= annual && u64::from(monthly) * 12 >= u64::from(annual));
    }
}

#[test]
fn the_death_scales_are_bounded_so_the_tables_stay_small() {
    refused_life(
        "certain_at = 105",
        "certain_at = 201",
        "death.certain_at",
        "at most",
    );
    refused_life(
        "infant_years = 2",
        "infant_years = 1001",
        "death.infant_years",
        "at most",
    );
    refused_life(
        "old_doubling = 8 ",
        "old_doubling = 1001 ",
        "death.old_doubling",
        "at most",
    );
}

#[test]
fn a_doubling_of_one_year_loads_with_exact_powers_of_any_width() {
    // R = 2: the powers are 2^k ppb-scaled, beyond 64 bits from k = 34 and beyond 128 from k = 99.
    let life = edit(life_text(), "old_doubling = 8 ", "old_doubling = 1 ");
    let rules = parse(&life, names_text()).expect("a doubling of one year is permitted");
    let death = &rules.death;
    let power = |k: u32| Big::new(1_000_000_000).mul(&Big::new(2).pow(k));
    assert_eq!(death.tables.exponent[10], power(10));
    assert_eq!(death.tables.exponent[85], power(85));
    assert_eq!(death.old_age_factor(20 + 85), power(85));
    assert_eq!(
        death.old_age_factor(0),
        Big::new(1_000_000_000_000_000_000).divmod(&power(20)).0
    );
    assert_eq!(
        death.annual_ppb(&neutral(100)),
        1_000_000_000,
        "capped, and no overflow"
    );
    assert_eq!(
        death.annual_ppb(&neutral(20)),
        7_509_079,
        "at old_from the old-age factor is 1 either way"
    );
    // A terminal age of 120 puts 2^99 x 10^9 in the table (above 128 bits), and it loads and is used exactly.
    let long = edit(&life, "certain_at = 105", "certain_at = 120");
    let rules = parse(&long, names_text()).expect("a long span of ages loads");
    let death = &rules.death;
    assert!(power(99).to_u128().is_none(), "the case is beyond 128 bits");
    assert_eq!(death.tables.exponent[99], power(99));
    assert_eq!(death.old_age_factor(119), power(99));
    assert_eq!(death.annual_ppb(&neutral(119)), 1_000_000_000);
    assert_eq!(death.annual_ppb(&neutral(20)), 7_509_079);
    // And the longest the loader allows, 200, with `old_from` 0.
    let longest = edit(
        &edit(&life, "certain_at = 105", "certain_at = 200"),
        "old_from = 20",
        "old_from = 0",
    );
    let death = &parse(&longest, names_text())
        .expect("the widest span loads")
        .death;
    assert_eq!(death.old_age_factor(199), power(199));
    assert_eq!(death.annual_ppb(&neutral(199)), 1_000_000_000);
}
