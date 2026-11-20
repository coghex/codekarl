//! Personality data contracts (CK-362): the shipped file loads, and each validation rule refuses the data it names.

use crate::{
    DataError, Personality, Rules,
    personality::{MANNER_PAIRS, VICE_PAIRS},
};
use std::{path::Path, sync::OnceLock};

fn shipped_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        // Read at run time, not with `include_str!`, so a crate built without the assets directory still compiles.
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/personality.toml"),
        )
        .expect("shipped personality data")
    })
}
fn registry() -> Vec<String> {
    let rules =
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .expect("shipped rules");
    rules
        .peoples
        .cultures
        .iter()
        .map(|c| c.id.clone())
        .collect()
}
fn parse(text: &str) -> Result<Personality, DataError> {
    let cultures = registry();
    let cultures: Vec<&str> = cultures.iter().map(String::as_str).collect();
    Personality::parse("personality.toml", text, &cultures)
}
/// The shipped text with the first `from` replaced by `to`; the shipped text must contain it.
fn edit(from: &str, to: &str) -> String {
    assert!(
        shipped_text().contains(from),
        "the shipped text no longer contains {from:?}"
    );
    shipped_text().replacen(from, to, 1)
}
fn refused(text: &str, entry: &str, problem: &str) {
    let error = parse(text).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert_eq!(error.file, "personality.toml");
}

#[test]
fn the_shipped_file_loads_with_the_values_it_states() {
    let rules = parse(shipped_text()).expect("shipped personality");
    assert_eq!(rules.vice_spread, 2400);
    assert_eq!(rules.manner_spread, 6400);
    assert_eq!(rules.manner_range, [-20_000, 20_000]);
    assert_eq!(rules.parents_weight, 500);
    assert_eq!(rules.drift_per_point, 10);
    assert_eq!(rules.yearly_keep, 950);
    assert_eq!(rules.stress.monthly_recovery, 56);
    assert_eq!(rules.stress.recovery_per_black, -100);
    assert_eq!(rules.stress.indulge_relief, 15_000);
    assert_eq!(rules.stress.confess_step, 500);
    assert_eq!(rules.opinion.compat_manner_max, 10_000);
    assert_eq!(rules.opinion.honour_weight, 100);
    assert_eq!(
        rules.opinion.drunkard_kinship,
        [5000, 10_000, 15_000, 20_000, 30_000, 40_000]
    );
    assert_eq!(rules.opinion.fool_in_power[5], -15_000);
    // blood leans to lust and gluttony, yellow bile to wrath and pride, black bile to envy, phlegm to sloth.
    assert_eq!(
        rules.drift_humour,
        [Some(1), None, Some(0), Some(2), Some(0), Some(1), Some(3)]
    );
    assert_eq!(rules.culture_means.len(), 21);
    assert!(
        rules
            .culture_means
            .values()
            .all(|means| means.iter().all(|m| *m == 0))
    );
    // The same data loads through the rules.
    let loaded =
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .expect("rules");
    assert_eq!(loaded.personality, rules);
}

#[test]
fn a_culture_mean_is_read_exactly_and_any_pair_may_carry_one() {
    let text = edit(
        "ulaid = {}",
        "ulaid = { pride_humility = -1.5, harsh_merciful = 24.125 }",
    );
    let rules = parse(&text).expect("a mean");
    let means = &rules.culture_means["ulaid"];
    assert_eq!((means[0], means[15]), (-1500, 24_125));
    assert_eq!(means[1..15], [0; 14]);
}

#[test]
fn a_missing_or_misplaced_pair_in_a_roster_is_refused() {
    // a missing vice pair
    refused(
        &edit(r#", "sloth_diligence"]"#, "]"),
        "vice_virtue.pairs",
        "sloth_diligence",
    );
    // a manner pair in the vice roster
    refused(
        &edit(r#""sloth_diligence"]"#, r#""guarded_open"]"#),
        "vice_virtue.pairs",
        "guarded_open",
    );
    // a repeated pair
    refused(
        &edit(r#""greed_charity","#, r#""pride_humility","#),
        "vice_virtue.pairs",
        "repeated",
    );
    // a misplaced pair: the order is the data order
    refused(
        &edit(
            r#""pride_humility", "greed_charity""#,
            r#""greed_charity", "pride_humility""#,
        ),
        "vice_virtue.pairs",
        "order",
    );
    // the manner roster is exact too
    refused(
        &edit(
            r#""guarded_open", "deceitful_honest""#,
            r#""deceitful_honest", "guarded_open""#,
        ),
        "manner.pairs",
        "order",
    );
    refused(
        &edit(r#""harsh_merciful"]"#, r#""pride_humility"]"#),
        "manner.pairs",
        "pride_humility",
    );
}

#[test]
fn drift_keys_and_targets_are_validated() {
    // a manner pair as a drift target
    refused(
        &edit(r#"["envy_kindness"]"#, r#"["guarded_open"]"#),
        "humour_drift.black_bile",
        "guarded_open",
    );
    // an unknown humour
    refused(
        &edit("black_bile = [", "blue_bile = ["),
        "humour_drift.blue_bile",
        "humours",
    );
    // a vice under two humours
    refused(
        &edit(
            r#"phlegm = ["sloth_diligence"]"#,
            r#"phlegm = ["envy_kindness"]"#,
        ),
        "humour_drift.phlegm",
        "already drifts",
    );
}

#[test]
fn ranges_are_the_simulations_fixed_bounds() {
    refused(
        &edit("range = [-7, 7]", "range = [-6, 7]"),
        "vice_virtue.range",
        "[-7, 7]",
    );
    refused(
        &edit("range = [0, 100]", "range = [100, 0]"),
        "stress.range",
        "[0, 100]",
    );
    refused(
        &edit("range = [-20, 20]", "range = [20, -20]"),
        "manner.range",
        "low end",
    );
    refused(
        &edit("range = [-20, 20]", "range = [5, 5]"),
        "manner.range",
        "low end",
    );
    // the manner range is a display range: any ordered pair loads
    assert_eq!(
        parse(&edit("range = [-20, 20]", "range = [-30, 30]"))
            .expect("display range")
            .manner_range,
        [-30_000, 30_000]
    );
}

#[test]
fn each_value_outside_its_domain_is_refused() {
    let cases = [
        ("spread = 2.4 ", "spread = -2.4 ", "vice_virtue.spread"),
        ("spread = 6.4 ", "spread = -6.4 ", "manner.spread"),
        (
            "parents_weight = 0.5 ",
            "parents_weight = 1.5 ",
            "birth.parents_weight",
        ),
        (
            "parents_weight = 0.5 ",
            "parents_weight = -0.5 ",
            "birth.parents_weight",
        ),
        (
            "monthly_recovery = 0.056 ",
            "monthly_recovery = 1.5 ",
            "stress.monthly_recovery",
        ),
        (
            "yearly_keep = 0.95 ",
            "yearly_keep = 1.5 ",
            "standing.yearly_keep",
        ),
        (
            "events_per_point = 0.05 ",
            "events_per_point = -0.05 ",
            "stress.events_per_point",
        ),
        (
            "indulge_relief = 15 ",
            "indulge_relief = -15 ",
            "stress.indulge_relief",
        ),
        (
            "confess_relief = 10 ",
            "confess_relief = -10 ",
            "stress.confess_relief",
        ),
        (
            "indulge_step = 1.0",
            "indulge_step = -1.0",
            "stress.indulge_step",
        ),
        (
            "confess_step = 0.5",
            "confess_step = -0.5",
            "stress.confess_step",
        ),
        (
            "per_point = 0.01       #",
            "per_point = -0.01       #",
            "humour_drift.per_point",
        ),
        (
            "penalty_per_point = 0.01 ",
            "penalty_per_point = -0.01 ",
            "stress.penalty_per_point",
        ),
        (
            "penalty_floor = 0.5 ",
            "penalty_floor = 1.5 ",
            "stress.penalty_floor",
        ),
        (
            "compat_manner_max = 10 ",
            "compat_manner_max = -10 ",
            "opinion.compat_manner_max",
        ),
        (
            "compat_humour_max = 5 ",
            "compat_humour_max = -5 ",
            "opinion.compat_humour_max",
        ),
        (
            "standing_cap = 20 ",
            "standing_cap = -20 ",
            "opinion.standing_cap",
        ),
    ];
    for (from, to, entry) in cases {
        refused(&edit(from, to), entry, "");
    }
}

#[test]
fn grade_lists_and_signs_are_validated() {
    // a grade list of five or seven entries, and a fractional grade term
    refused(
        &edit(
            "beauty = [0, 5, 10, 15, 20, 25]",
            "beauty = [0, 5, 10, 15, 20]",
        ),
        "opinion.beauty",
        "six",
    );
    refused(
        &edit(
            "madness = [-2, -4, -6, -8, -10, -12]",
            "madness = [-2, -4, -6, -8, -10, -12, -14]",
        ),
        "opinion.madness",
        "six",
    );
    refused(
        &edit(
            "beauty = [0, 5, 10, 15, 20, 25]",
            "beauty = [0, 5, 10, 15, 20, 25.5]",
        ),
        "opinion.beauty",
        "",
    );
    // factors and weights may have either sign
    parse(&edit("honour_weight = 0.10 ", "honour_weight = -0.10 ")).expect("a negative weight");
    parse(&edit(
        "recovery_per_blood = 0.10 ",
        "recovery_per_blood = -0.10 ",
    ))
    .expect("a negative factor");
}

#[test]
fn the_culture_table_is_exactly_the_registry() {
    refused(&edit("ulaid = {}\n", ""), "culture_means.ulaid", "missing");
    refused(
        &edit("kentish = {}", "kentish = {}\nnarnia = {}"),
        "culture_means.narnia",
        "registry",
    );
    refused(
        &edit("ulaid = {}", "ulaid = { piety = 1 }"),
        "culture_means.ulaid.piety",
        "known pair",
    );
}

#[test]
fn unknown_sections_and_keys_and_missing_values_are_refused() {
    refused(
        &format!("{}\n[extra]\n", shipped_text()),
        "extra",
        "unknown",
    );
    refused(&edit("[birth]", "[birthright]"), "birth", "");
    refused(
        &edit("yearly_keep = 0.95 ", "yearly_kept = 0.95 "),
        "standing.yearly_kept",
        "unknown",
    );
    refused(
        &edit("fame_weight = 0.05 ", "fame_weigth = 0.05 "),
        "opinion",
        "",
    );
}

#[test]
fn the_pair_lists_are_the_data_order() {
    assert_eq!(
        VICE_PAIRS.len() + MANNER_PAIRS.len(),
        crate::personality::PAIR_COUNT
    );
    let text = shipped_text();
    let (mut last, mut found) = (0, 0);
    for name in VICE_PAIRS.iter().chain(&MANNER_PAIRS) {
        let at = text[last..].find(name).expect("in file order") + last;
        last = at;
        found += 1;
    }
    assert_eq!(found, 16);
}
