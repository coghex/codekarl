//! Life-cycle data contracts (CK-364): the shipped files load and state what they say, and each validation rule refuses the data it
//! names.

use crate::{
    DataError, LifeCycle, Rules,
    lifecycle::{Ages, Files, Registry},
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
fn epithets_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| asset("epithets.toml"))
}
fn shipped_rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .expect("shipped rules")
    })
}
fn parse(life: &str, names: &str) -> Result<LifeCycle, DataError> {
    parse_all(life, names, epithets_text())
}
fn parse_all(life: &str, names: &str, epithets: &str) -> Result<LifeCycle, DataError> {
    let rules = shipped_rules();
    let pop_types: Vec<&str> = rules.pop_types.iter().map(|p| p.id.as_str()).collect();
    LifeCycle::parse(
        &Files {
            life: ("life-cycle.toml", life),
            names: ("names.toml", names),
            epithets: ("epithets.toml", epithets),
        },
        &Registry {
            events: &rules.events,
            genetics: &rules.genetics,
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

// ---------------------------------------------------------------------------------------------------------------------
// Epithets and namesakes (CK-365, characters slice 5b).

use crate::{EpithetScope, EpithetTrigger};

fn refused_epithets(from: &str, to: &str, entry: &str, problem: &str) {
    let error = parse_all(life_text(), names_text(), &edit(epithets_text(), from, to)).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert_eq!(error.file, "epithets.toml");
}

#[test]
fn the_shipped_epithets_load_in_file_order_ten_of_them_gainable() {
    let epithets = &shipped_rules().lifecycle.epithets;
    assert_eq!(epithets.len(), 30);
    // The file's own order, not the key order: `frossach` is first, `gluindub` the third.
    let keys: Vec<&str> = epithets.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(&keys[..4], ["frossach", "caech", "gluindub", "of_place"]);
    assert_eq!(keys.last(), Some(&"the_monk"));
    let gainable: Vec<&str> = epithets
        .iter()
        .filter(|e| e.trigger != EpithetTrigger::Later)
        .map(|e| e.key.as_str())
        .collect();
    assert_eq!(
        gainable,
        [
            "gluindub",
            "find",
            "becc",
            "the_wise",
            "the_bold",
            "the_blind",
            "the_lame",
            "the_fat",
            "the_old",
            "the_pilgrim"
        ]
    );
    let by = |key: &str| epithets.iter().find(|e| e.key == key).expect(key);
    // The three kinds of scope.
    assert_eq!(by("the_wise").scope, EpithetScope::Universal);
    assert_eq!(
        by("find").scope,
        EpithetScope::Subgroup("gaelic".to_owned())
    );
    assert_eq!(
        by("the_pilgrim").scope,
        EpithetScope::Religion("christianity".to_owned())
    );
    assert_eq!(
        by("the_pilgrim").trigger,
        EpithetTrigger::Happening("pilgrimage".to_owned())
    );
    assert_eq!(
        (by("find").importance, by("find").chance_ppb),
        (20, 200_000_000)
    );
    assert_eq!(by("gluindub").chance_ppb, 300_000);
    // The four keys beyond slice 4's vocabulary, and `adult`.
    assert_eq!(by("find").when.grade_min, [("beauty".to_owned(), 2)]);
    assert_eq!(by("the_fat").when.weight_ratio_min, Some(1300));
    assert_eq!(
        by("the_bold").when.count_min,
        [("battles_survived".to_owned(), 3)]
    );
    assert!(by("becc").when.adult);
    // Height is the first genetic stat; the limits are in thousandths, a man's then a woman's.
    assert_eq!(by("becc").when.stat_max_by_sex, [(0, 160_000, 150_000)]);
}

#[test]
fn the_shipped_namesake_order_loads() {
    let namesakes = &shipped_rules().lifecycle.namesakes;
    assert_eq!(
        namesakes.order,
        [
            ("father".to_owned(), 100_000_000),
            ("grandfather".to_owned(), 100_000_000),
            ("uncle".to_owned(), 60_000_000),
            ("fathers_friend".to_owned(), 40_000_000),
            ("ancestor".to_owned(), 50_000_000),
        ]
    );
}

#[test]
fn each_epithet_field_error_names_its_entry() {
    for (from, to, entry, problem) in [
        // An unknown key and a bad value.
        (
            "chance = 0.0003",
            "chance = 0.0003\nbogus = 1",
            "epithet.gluindub.bogus",
            "unknown key",
        ),
        (
            "chance = 0.0003",
            "chance = 1.5",
            "epithet.gluindub.chance",
            "0..1",
        ),
        (
            "importance = 30\nscope = \"gaelic\"\nfrom = \"event: a mark",
            "importance = -30\nscope = \"gaelic\"\nfrom = \"event: a mark",
            "epithet.gluindub.importance",
            "whole number",
        ),
        // A `later` epithet has neither `when` nor `chance`.
        (
            "trigger = \"later\"",
            "trigger = \"later\"\nwhen = {}",
            "epithet.frossach.when",
            "`later`",
        ),
        (
            "trigger = \"later\"",
            "trigger = \"later\"\nchance = 0.1",
            "epithet.frossach.chance",
            "`later`",
        ),
        // A gainable epithet needs both.
        (
            "when = {}\nchance = 0.0003",
            "chance = 0.0003",
            "epithet.gluindub.when",
            "missing value",
        ),
        (
            "when = {}\nchance = 0.0003",
            "when = {}",
            "epithet.gluindub.chance",
            "missing value",
        ),
    ] {
        refused_epithets(from, to, entry, problem);
    }
}

#[test]
fn each_epithet_scope_trigger_and_condition_error_names_its_entry() {
    for (from, to, entry, problem) in [
        // The scope and the trigger.
        (
            "scope = \"gaelic\"",
            "scope = \"nowhere\"",
            "epithet.frossach.scope",
            "not `universal`",
        ),
        (
            "trigger = \"later\"",
            "trigger = \"monthly\"",
            "epithet.frossach.trigger",
            "`yearly`",
        ),
        (
            "trigger = \"happening:pilgrimage\"",
            "trigger = \"happening:\"",
            "epithet.the_pilgrim.trigger",
            "`happening:<name>`",
        ),
        // The vocabulary.
        (
            "when = {}\nchance = 0.0003",
            "when = { mood = 1 }\nchance = 0.0003",
            "epithet.gluindub.when.mood",
            "unknown key",
        ),
        (
            "grade_min = { beauty = 2 }",
            "grade_min = { beauty = 7 }",
            "grade_min.beauty",
            "1 (C) to 6",
        ),
        (
            "grade_min = { beauty = 2 }",
            "grade_min = { blind = 2 }",
            "grade_min.blind",
            "not a graded trait",
        ),
        (
            "grade_min = { beauty = 2 }",
            "grade_min = { nonsense = 2 }",
            "grade_min.nonsense",
            "not a key of the catalog",
        ),
        (
            "count_min = { battles_survived = 3 }",
            "count_min = { duels = 3 }",
            "count_min.duels",
            "not a counter",
        ),
        (
            "stat_max_by_sex = { height = { m = 160, f = 150 } }",
            "stat_max_by_sex = { girth = { m = 160, f = 150 } }",
            "stat_max_by_sex.girth",
            "not a genetic stat",
        ),
        (
            "stat_max_by_sex = { height = { m = 160, f = 150 } }",
            "stat_max_by_sex = { height = { m = 160 } }",
            "stat_max_by_sex.height.f",
            "missing value",
        ),
        (
            "weight_ratio_min = 1.3",
            "weight_ratio_min = 0",
            "weight_ratio_min",
            "above 0",
        ),
        (
            "adult = true",
            "adult = false",
            "when.adult",
            "only ever true",
        ),
        (
            "skill_min = { scholarship = 18 }",
            "skill_min = { nonsense = 18 }",
            "skill_min.nonsense",
            "not in `[skills].list`",
        ),
    ] {
        refused_epithets(from, to, entry, problem);
    }
}

#[test]
fn an_epithet_scope_that_is_both_a_subgroup_and_a_religion_is_refused() {
    // `gaelic` is a subgroup; a religion of the same key makes the scope ambiguous.
    let rules = shipped_rules();
    let pop_types: Vec<&str> = rules.pop_types.iter().map(|p| p.id.as_str()).collect();
    let mut religions = rules.religions.clone();
    religions.push("gaelic".to_owned());
    let error = LifeCycle::parse(
        &Files {
            life: ("life-cycle.toml", life_text()),
            names: ("names.toml", names_text()),
            epithets: ("epithets.toml", epithets_text()),
        },
        &Registry {
            events: &rules.events,
            genetics: &rules.genetics,
            peoples: &rules.peoples,
            pop_types: &pop_types,
            religions: &religions,
        },
    )
    .unwrap_err();
    assert!(
        error
            .problem
            .contains("both a culture subgroup and a religion"),
        "{error:?}"
    );
}

#[test]
fn each_namesake_validation_error_names_its_entry() {
    for (from, to, problem) in [
        (
            "[\"uncle\", 0.06]",
            "[\"cousin\", 0.06]",
            "not a kind of namesake",
        ),
        ("[\"uncle\", 0.06]", "[\"father\", 0.06]", "listed twice"),
        ("[\"uncle\", 0.06]", "[\"uncle\", 1.5]", "0..1"),
        ("[\"uncle\", 0.06]", "[\"uncle\"]", "`[kind, chance]`"),
    ] {
        refused_names(from, to, "namesakes.order", problem);
    }
    refused_names(
        "order = ",
        "weights = [] \norder = ",
        "namesakes.weights",
        "unknown key",
    );
}

#[test]
fn the_marriage_tables_load_with_what_they_state() {
    let marriage = &shipped_rules().lifecycle.marriage;
    let wish = &marriage.wish;
    assert_eq!(wish.women.len(), 7);
    assert_eq!(wish.women[0], (12, 50_000_000));
    assert_eq!(wish.women[2], (20, 400_000_000));
    assert_eq!(wish.women.last(), Some(&(60, 0)));
    assert_eq!(wish.men.len(), 7);
    assert_eq!(wish.men[0], (14, 20_000_000));
    assert_eq!(wish.men.last(), Some(&(70, 0)));
    assert_eq!(
        (wish.lust_per_point, wish.gregarious_per_point),
        (-50_000, 20_000)
    );
    assert_eq!(
        wish.pop_type.iter().collect::<Vec<_>>(),
        [(&"monk".to_owned(), &300_000_000)]
    );
    let pool = &marriage.pool;
    assert_eq!(
        (pool.near_m, pool.far_m, pool.min_candidates),
        (50_000, 150_000, 3)
    );
    let score = &marriage.score;
    assert_eq!(
        score
            .rank
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect::<Vec<_>>(),
        [("labourer", 1), ("monk", 2), ("noble", 3), ("peasant", 1)]
    );
    assert_eq!(
        (score.rank_gap, score.age_gap_free, score.age_gap),
        (-15_000, 10, -2_000)
    );
    assert_eq!((score.opinion, score.fame), (500_000, 200_000));
    assert_eq!(
        score
            .kin
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect::<Vec<_>>(),
        [
            ("first_cousin", -40_000),
            ("in_law", -10_000),
            ("second_cousin", -15_000)
        ]
    );
    assert_eq!(score.child_age_penalty, -100_000);
    assert_eq!(marriage.accept.base_ppb, 500_000_000);
    assert_eq!(marriage.accept.per_score_ppb, 10_000_000);
}

#[test]
fn the_marriage_tables_refuse_what_they_name() {
    // A bad curve: ages that do not increase, a negative chance, a chance outside 0..1, a point without a chance.
    refused_life(
        "women = [[12, 0.05], [15, 0.3]",
        "women = [[12, 0.05], [12, 0.3]",
        "marriage.wish.women",
        "strictly increasing",
    );
    refused_life(
        "men = [[14, 0.02], [17, 0.15]",
        "men = [[14, 0.02], [17, -0.15]",
        "marriage.wish.men",
        "must not be negative",
    );
    refused_life(
        "women = [[12, 0.05], [15, 0.3]",
        "women = [[12, 0.05], [15, 1.5]",
        "marriage.wish.women",
        "must lie in 0..1",
    );
    refused_life(
        "men = [[14, 0.02], [17, 0.15]",
        "men = [[14, 0.02], [17]",
        "marriage.wish.men",
        "a point is",
    );
    // A probability outside 0..1.
    refused_life("base = 0.5", "base = 1.5", "marriage.accept.base", "0..1");
    // An unknown pop type, in the wish and in the ranks; a rank left out.
    refused_life(
        "pop_type = { monk = 0.3 }",
        "pop_type = { cleric = 0.3 }",
        "marriage.wish.pop_type.cleric",
        "not a population type",
    );
    refused_life(
        "rank = { noble = 3, monk = 2, labourer = 1, peasant = 1 }",
        "rank = { noble = 3, monk = 2, labourer = 1 }",
        "marriage.score.rank.peasant",
        "missing value",
    );
    refused_life(
        "rank = { noble = 3, monk = 2, labourer = 1, peasant = 1 }",
        "rank = { noble = 3, monk = 2, labourer = 1, peasant = 1, cleric = 2 }",
        "marriage.score.rank.cleric",
        "unknown key",
    );
    // An unknown kinship key, and one left out.
    refused_life(
        "kin = { first_cousin = -40,",
        "kin = { cousin = -40,",
        "marriage.score.kin.cousin",
        "unknown key",
    );
    refused_life(
        "kin = { first_cousin = -40, second_cousin = -15, in_law = -10 }",
        "kin = { first_cousin = -40, second_cousin = -15 }",
        "marriage.score.kin.in_law",
        "missing value",
    );
    // The pool: its near reach may not exceed its far one; a distance is a positive whole number of kilometres.
    refused_life(
        "near_km = 50",
        "near_km = 200",
        "marriage.pool.near_km",
        "must not exceed far_km",
    );
    refused_life(
        "far_km = 150",
        "far_km = 0",
        "marriage.pool.far_km",
        "positive",
    );
    // A section or a key the contract does not have.
    refused_life(
        "[marriage.accept]",
        "[marriage.betrothal]",
        "marriage.betrothal",
        "unknown section",
    );
    refused_life(
        "per_score = 0.01",
        "per_score = 0.01\nflat = 1",
        "marriage.accept.flat",
        "unknown key",
    );
    refused_life(
        "age_gap_free = 10 ",
        "age_gap_free = -1 ",
        "marriage.score.age_gap_free",
        "whole number",
    );
}

// --- Upbringing and nutrition (CK-367) -------------------------------------------------------------------------------------------

#[test]
fn the_shipped_upbringing_and_nutrition_load_with_what_they_state() {
    let data = &shipped_rules().lifecycle;
    let up = &data.upbringing;
    assert_eq!(
        (
            up.to_age,
            up.pull_ppb,
            up.noise_vice_virtue,
            up.noise_manner
        ),
        (14, 40_000_000, 480, 1280)
    );
    let n = &data.nutrition;
    // Thousandths of a centimetre per permille, of a centimetre, thousandths of a permille; shares in millionths.
    assert_eq!(n.height_per_food_milli, 30);
    assert_eq!((n.height_low, n.height_high), (-8000, 2000));
    assert_eq!(n.height_reference_milli, 900_000);
    assert_eq!(
        (
            n.weight_base,
            n.weight_food,
            n.weight_rich,
            n.weight_gluttony
        ),
        (800_000, 200_000, 100_000, 10_000)
    );
}

#[test]
fn each_upbringing_domain_is_refused() {
    for (from, to, entry, problem) in [
        // `to_age` outside 1..18.
        ("to_age = 14", "to_age = 0", "upbringing.to_age", "1..18"),
        ("to_age = 14", "to_age = 19", "upbringing.to_age", "1..18"),
        (
            "to_age = 14",
            "to_age = -1",
            "upbringing.to_age",
            "whole number",
        ),
        // `pull` outside 0..1.
        ("pull = 0.04 ", "pull = 1.5 ", "upbringing.pull", "0..1"),
        ("pull = 0.04 ", "pull = -0.04 ", "upbringing.pull", "0..1"),
        // A negative noise spread.
        (
            "vice_virtue = 0.48",
            "vice_virtue = -0.48",
            "upbringing.noise.vice_virtue",
            "must not be negative",
        ),
        (
            "manner = 1.28",
            "manner = -1.28",
            "upbringing.noise.manner",
            "must not be negative",
        ),
    ] {
        refused_life(from, to, entry, problem);
    }
}

#[test]
fn each_nutrition_domain_is_refused_and_weight_monthly_is_admitted() {
    for (from, to, entry, problem) in [
        // `height_range` is `[low, high]` with `low <= 0 <= high` and `low < high`.
        (
            "height_range = [-8, 2]",
            "height_range = [1, 2]",
            "nutrition.height_range",
            "low <= 0 <= high",
        ),
        (
            "height_range = [-8, 2]",
            "height_range = [-8, -1]",
            "nutrition.height_range",
            "low <= 0 <= high",
        ),
        (
            "height_range = [-8, 2]",
            "height_range = [0, 0]",
            "nutrition.height_range",
            "low < high",
        ),
        (
            "height_range = [-8, 2]",
            "height_range = [2, -8]",
            "nutrition.height_range",
            "low <= 0 <= high",
        ),
        (
            "height_range = [-8, 2]",
            "height_range = [-8]",
            "nutrition.height_range",
            "[low, high]",
        ),
        // A negative figure.
        (
            "height_per_food_point = 0.03",
            "height_per_food_point = -0.03",
            "nutrition.height_per_food_point",
            "must not be negative",
        ),
        (
            "height_reference = 900",
            "height_reference = -900",
            "nutrition.height_reference",
            "must not be negative",
        ),
        (
            "weight_base = 0.80",
            "weight_base = -0.80",
            "nutrition.weight_base",
            "must not be negative",
        ),
        (
            "weight_food = 0.20",
            "weight_food = -0.20",
            "nutrition.weight_food",
            "must not be negative",
        ),
        (
            "weight_rich = 0.10",
            "weight_rich = -0.10",
            "nutrition.weight_rich",
            "must not be negative",
        ),
        (
            "weight_gluttony = 0.01",
            "weight_gluttony = -0.01",
            "nutrition.weight_gluttony",
            "must not be negative",
        ),
        // A key the contract does not have.
        (
            "weight_monthly = 0.0833",
            "weight_monthly = 0.0833\nweight_daily = 1",
            "nutrition.weight_daily",
            "unknown key",
        ),
    ] {
        refused_life(from, to, entry, problem);
    }
    // `weight_monthly` is admitted whatever it says, and not required: the twelfth is exact.
    let ignored = |to: &str| {
        parse(
            &edit(life_text(), "weight_monthly = 0.0833", to),
            names_text(),
        )
        .expect("admitted")
    };
    let shipped = &shipped_rules().lifecycle;
    assert_eq!(ignored("weight_monthly = 7").nutrition, shipped.nutrition);
    assert_eq!(ignored("").nutrition, shipped.nutrition);
}

#[test]
fn a_weight_coefficient_is_any_non_negative_value_the_representation_holds() {
    // No cap at 1000 (CK-367 §0 names only the sign): a coefficient above it loads, and one beyond the representation is refused.
    for key in [
        "weight_base = 0.80",
        "weight_food = 0.20",
        "weight_rich = 0.10",
        "weight_gluttony = 0.01",
    ] {
        let name = key.split(' ').next().unwrap();
        let big = parse(
            &edit(life_text(), key, &format!("{name} = 1000.000001")),
            names_text(),
        )
        .expect("a coefficient above 1000 loads")
        .nutrition;
        let got = match name {
            "weight_base" => big.weight_base,
            "weight_food" => big.weight_food,
            "weight_rich" => big.weight_rich,
            _ => big.weight_gluttony,
        };
        assert_eq!(got, 1_000_000_001, "{name}");
        refused_life(
            key,
            &format!("{name} = 99999999999999999"),
            name,
            "too large",
        );
    }
}
