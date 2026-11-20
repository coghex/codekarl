//! Genetic data and pop state contracts (CK-361): loading, validation, seeding, blending and the arithmetic references.

use crate::{
    DataError, Genetics, PopGenetics, Rules,
    genetics::{
        ATTRIBUTE_COUNT, ATTRIBUTES, Dominance, Mode, PPB, ShowsIn, attribute_index,
        clamp_attribute, round_div, spread_factor,
    },
};
use ck_sim::Canonical;
use std::{collections::BTreeMap, path::Path, sync::OnceLock};

fn asset(name: &str) -> &'static str {
    static FILES: OnceLock<std::sync::Mutex<BTreeMap<String, &'static str>>> = OnceLock::new();
    let mut files = FILES.get_or_init(Default::default).lock().expect("lock");
    files.entry(name.to_owned()).or_insert_with(|| {
        // Read at run time, not with `include_str!`, so a crate built without the assets directory still compiles.
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/game/genetics")
            .join(name);
        Box::leak(
            std::fs::read_to_string(path)
                .expect("shipped genetic data")
                .into_boxed_str(),
        )
    })
}
fn haplogroups() -> &'static str {
    asset("haplogroups.toml")
}
fn traits() -> &'static str {
    asset("traits.toml")
}

fn shipped_rules() -> Rules {
    Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
        .unwrap()
}
fn parse(haplogroups: &str, traits: &str, cultures: &[&str]) -> Result<Genetics, DataError> {
    Genetics::parse(
        ("haplogroups.toml", haplogroups),
        ("traits.toml", traits),
        cultures,
    )
}
/// `text` with the first `from` replaced by `to`; the original must contain it.
fn edit(text: &str, from: &str, to: &str) -> String {
    assert!(
        text.contains(from),
        "the shipped text no longer contains {from:?}"
    );
    text.replacen(from, to, 1)
}
fn refused(haplogroups: &str, traits: &str, cultures: &[&str], entry: &str, problem: &str) {
    let error = parse(haplogroups, traits, cultures).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert!(error.file == "haplogroups.toml" || error.file == "traits.toml");
}

// --- loading ---------------------------------------------------------------------------------------------------------

#[test]
fn both_files_load_as_shipped_with_every_culture_mapped() {
    let rules = shipped_rules();
    let genetics = &rules.genetics;
    assert!(!genetics.is_empty());
    for culture in &rules.peoples.cultures {
        assert!(genetics.start_set(&culture.id).is_some(), "{}", culture.id);
        assert!(genetics.seed_pop(&culture.id).is_some(), "{}", culture.id);
    }
    assert_eq!(rules.peoples.cultures.len(), 21);
    assert_eq!(genetics.graded().len(), 6);
    assert_eq!(genetics.mendelian().len(), 12);
    let defaults = genetics.graded()["beauty"];
    assert_eq!(
        (
            defaults.w,
            defaults.d,
            defaults.s0,
            defaults.k,
            defaults.m,
            defaults.q,
            defaults.max
        ),
        (500, 400, 300, 300, 10_000_000, 300_000_000, 6)
    );
    assert_eq!(defaults.founder, 60_000_000);
    assert_eq!(defaults.founder_q, 450_000_000);
    let madness = genetics.graded()["madness"];
    assert_eq!(
        (madness.d, madness.m, madness.founder),
        (800, 2_000_000, 5_000_000)
    );
}

#[test]
fn rare_rates_are_held_exactly_in_parts_per_billion() {
    let genetics = shipped_rules().genetics;
    assert_eq!(genetics.mendelian()["dwarfism"].allele, 100_000);
    assert_eq!(genetics.mendelian()["bleeder"].allele, 500_000);
    assert_eq!(genetics.mendelian()["bleeder"].mode, Mode::X);
    assert_eq!(genetics.mendelian()["twin_bearer"].shows_in, ShowsIn::Women);
    assert_eq!(genetics.mendelian()["twin_bearer"].penetrance, PPB);
    for weak in ["deaf", "blind"] {
        let rule = genetics.mendelian()[weak];
        assert_eq!(
            (rule.mode, rule.dominance),
            (Mode::Ordinary, Dominance::Dominant)
        );
        assert_eq!(rule.penetrance, 100_000_000, "{weak} is a weak dominant");
    }
    assert_eq!(genetics.mendelian()["deaf"].allele, 5_000_000);
    assert_eq!(genetics.mendelian()["blind"].allele, 2_000_000);
}

#[test]
fn every_haplogroup_file_validation_error_is_refused_with_a_named_entry() {
    let cultures = ["ulaid"];
    // Haplogroup file.
    refused(
        haplogroups(),
        traits(),
        &["ulaid", "atlantean"],
        "culture_start.atlantean",
        "no start set",
    );
    refused(
        &edit(
            haplogroups(),
            "paternal = { R1b = 95, other = 5 }",
            "paternal = { R1c = 95, other = 5 }",
        ),
        traits(),
        &cultures,
        "start.irish.paternal.R1c",
        "undeclared haplogroup",
    );
    refused(
        &edit(
            haplogroups(),
            "paternal = { R1b = 95, other = 5 }",
            "paternal = { R1b = 90, other = 5 }",
        ),
        traits(),
        &cultures,
        "start.irish.paternal",
        "sum to 95",
    );
    refused(
        &edit(
            haplogroups(),
            "constitution = { mean = 11 }",
            "constitooshun = { mean = 11 }",
        ),
        traits(),
        &cultures,
        "constitooshun",
        "not a stat or humour",
    );
    refused(
        &edit(
            haplogroups(),
            "blood = { mean = 6 }",
            "bloood = { mean = 6 }",
        ),
        traits(),
        &cultures,
        "bloood",
        "not a stat or humour",
    );
    refused(
        &edit(
            haplogroups(),
            "prodigious_memory = 1.5",
            "prodigal_memory = 1.5",
        ),
        traits(),
        &cultures,
        "prodigal_memory",
        "not a trait",
    );
    refused(
        &edit(
            haplogroups(),
            "prodigious_memory = 1.5",
            "prodigious_memory = -1.5",
        ),
        traits(),
        &cultures,
        "prodigious_memory",
        "must not be negative",
    );
    refused(
        &edit(haplogroups(), "height = 0.8", "height = 1.5"),
        traits(),
        &cultures,
        "heritability.height",
        "0..1",
    );
    refused(
        &edit(
            haplogroups(),
            "height = { mean = 170, sd = 6 }",
            "height = { mean = 170, sd = -6 }",
        ),
        traits(),
        &cultures,
        "default.height.sd",
        "must not be negative",
    );
}

#[test]
fn every_trait_file_validation_error_is_refused_with_a_named_entry() {
    let cultures = ["ulaid"];
    refused(
        haplogroups(),
        &edit(traits(), "mode = \"x\"", "mode = \"z\""),
        &cultures,
        "bleeder.mode",
        "unknown mode",
    );
    refused(
        haplogroups(),
        &edit(
            traits(),
            "dominance = \"recessive\"",
            "dominance = \"semi\"",
        ),
        &cultures,
        "dominance",
        "unknown dominance",
    );
    refused(
        haplogroups(),
        &edit(traits(), "allele = 0.2", "allele = 1.5"),
        &cultures,
        "left_handed.allele",
        "0..1",
    );
    refused(
        haplogroups(),
        &edit(traits(), "allele = 0.0001", "allele = 0.0000000001"),
        &cultures,
        "dwarfism.allele",
        "more than 9 decimal places",
    );
    refused(
        haplogroups(),
        &edit(traits(), "allele = 0.2", "allele = 2e-1"),
        &cultures,
        "left_handed.allele",
        "not a plain decimal",
    );
    for (from, to, entry, problem) in [
        ("\nw = 0.5\n", "\nw = 1.5\n", "defaults.graded.w", "0..1"),
        (
            "\nd = 0.4\n",
            "\nd = -0.4\n",
            "defaults.graded.d",
            "must not be negative",
        ),
        (
            "\ns0 = 0.3\n",
            "\ns0 = -0.3\n",
            "defaults.graded.s0",
            "must not be negative",
        ),
        (
            "\nk = 0.3\n",
            "\nk = -0.3\n",
            "defaults.graded.k",
            "must not be negative",
        ),
        ("\nq = 0.3\n", "\nq = 1.5\n", "defaults.graded.q", "0..1"),
        (
            "\nfounder_q = 0.45\n",
            "\nfounder_q = 2.0\n",
            "defaults.graded.founder_q",
            "0..1",
        ),
        ("\nmax = 6\n", "\nmax = 7\n", "defaults.graded.max", "1..6"),
        ("\nmax = 6\n", "\nmax = 0\n", "defaults.graded.max", "1..6"),
    ] {
        refused(
            haplogroups(),
            &edit(traits(), from, to),
            &cultures,
            entry,
            problem,
        );
    }
}

#[test]
fn a_malformed_or_incomplete_file_is_refused() {
    refused("[default", traits(), &[], "<document>", "");
    refused(
        haplogroups(),
        "[mendelian.x]\nmode = \"ordinary\"\n",
        &[],
        "mendelian.x.dominance",
        "missing",
    );
    refused(
        &edit(haplogroups(), "[female]", "[famale]"),
        traits(),
        &[],
        "famale",
        "unknown section",
    );
    let without_phlegm = haplogroups().replace("phlegm = 0.5\n", "");
    refused(
        &without_phlegm,
        traits(),
        &[],
        "heritability.phlegm",
        "missing",
    );
}

// --- arithmetic references -------------------------------------------------------------------------------------------

#[test]
fn the_spread_factor_is_the_exact_integer_root() {
    // 2 f^2 <= 2,000,000 - h^2, the largest f.
    assert_eq!(spread_factor(800), 824);
    assert_eq!(spread_factor(1), 999, "not truncated to 1000 early");
    assert_eq!(spread_factor(0), 1000, "an exact square boundary");
    assert_eq!(spread_factor(1000), 707);
    assert_eq!(spread_factor(250), 984);
    assert_eq!(spread_factor(200), 989);
    for h in 0..=1000_u64 {
        let f = u64::from(spread_factor(u32::try_from(h).unwrap()));
        assert!(
            2 * f * f <= 2_000_000 - h * h && 2 * (f + 1) * (f + 1) > 2_000_000 - h * h,
            "h = {h}"
        );
    }
    let genetics = shipped_rules().genetics;
    let height = attribute_index("height").unwrap();
    let longevity = attribute_index("longevity").unwrap();
    assert_eq!(
        (genetics.spread(height), genetics.spread(longevity)),
        (824, 984)
    );
}

#[test]
fn rounding_goes_to_the_nearest_with_ties_away_from_zero() {
    assert_eq!(
        [
            round_div(5, 2),
            round_div(-5, 2),
            round_div(3, 2),
            round_div(1, 3),
            round_div(2, 3)
        ],
        [3, -3, 2, 0, 1]
    );
    assert_eq!(
        [
            round_div(-1, 2),
            round_div(-1, 3),
            round_div(-2, 3),
            round_div(0, 7)
        ],
        [-1, 0, -1, 0]
    );
}

#[test]
fn values_are_clamped_to_their_bounds() {
    let humour = attribute_index("blood").unwrap();
    assert_eq!(
        (clamp_attribute(humour, -5), clamp_attribute(humour, 10_001)),
        (0, 10_000)
    );
    assert_eq!(
        (clamp_attribute(2, 999), clamp_attribute(2, 99_000)),
        (1_000, 99_000)
    );
    assert_eq!(
        (clamp_attribute(0, 99_999), clamp_attribute(1, 29_999)),
        (100_000, 30_000)
    );
}

// --- seeding ---------------------------------------------------------------------------------------------------------

#[test]
fn an_irish_and_an_english_pop_are_seeded_from_their_start_sets() {
    let genetics = shipped_rules().genetics;
    let irish = genetics.seed_pop("ulaid").unwrap();
    let blood = attribute_index("blood").unwrap();
    let constitution = attribute_index("constitution").unwrap();
    assert_eq!(
        (irish.mean[blood], irish.mean[constitution]),
        (5475, 10_475)
    );
    assert_eq!(
        irish.paternal,
        BTreeMap::from([("R1b".to_owned(), 950), ("other".to_owned(), 50)])
    );
    assert_eq!(
        irish.maternal,
        BTreeMap::from([("H".to_owned(), 950), ("other".to_owned(), 50)])
    );
    assert_eq!(irish.founder["prodigious_memory"], 59_000_000);
    assert_eq!(
        irish.founder["beauty"], 60_000_000,
        "no multiplier: the data's founder rate"
    );
    assert_eq!(irish.carrier["dwarfism"], 100_000);
    let english = genetics.seed_pop("mercian").unwrap();
    assert_eq!(english.mean[0], 170_400);
    assert_eq!(
        (english.mean[blood], english.mean[constitution]),
        (5275, 10_375)
    );
    assert_eq!(english.paternal["I1"], 200);
    assert_eq!(english.founder["prodigious_memory"], 55_000_000);
    assert_eq!(english.sd, irish.sd, "no table overrides a spread");
    assert!(genetics.seed_pop("nowhere").is_none());
}

fn mini(paternal: &str, maternal: &str, start: &str, traits: &str) -> Genetics {
    let mut text = String::from("[default]\n");
    text.extend(
        ATTRIBUTES
            .iter()
            .map(|name| format!("{name} = {{ mean = 10, sd = 6 }}\n")),
    );
    text.push_str("[heritability]\n");
    text.extend(ATTRIBUTES.iter().map(|name| format!("{name} = 0.5\n")));
    text.push_str(paternal);
    text.push_str(maternal);
    text.push_str(start);
    text.push_str("[culture_start]\nc = \"s\"\n");
    parse(&text, traits, &["c"]).unwrap_or_else(|e| panic!("{e:?}"))
}

const CONDITION: &str =
    "[mendelian.t]\nmode = \"ordinary\"\ndominance = \"dominant\"\npenetrance = 1.0\n";
const ONE_TO_ONE: &str = "[start.s]\npaternal = { A = 100 }\nmaternal = { B = 100 }\n";

fn carrier_rate(paternal_multiplier: &str, maternal_multiplier: &str, allele: &str) -> u32 {
    let paternal = format!("[paternal.A]\n[paternal.A.trait_rates]\nt = {paternal_multiplier}\n");
    let maternal = format!("[maternal.B]\n[maternal.B.trait_rates]\nt = {maternal_multiplier}\n");
    let traits = format!("{CONDITION}allele = {allele}\n");
    mini(&paternal, &maternal, ONE_TO_ONE, &traits)
        .seed_pop("c")
        .unwrap()
        .carrier["t"]
}

#[test]
fn seeded_rates_keep_exact_intermediates_and_cap_only_the_final_value() {
    assert_eq!(
        carrier_rate("3.0", "2.0", "0.3"),
        1_000_000_000,
        "0.3 x 3.0 x 2.0 = 1.8, capped once at the end"
    );
    assert_eq!(
        carrier_rate("3.0", "2.0", "0.1"),
        600_000_000,
        "0.1 x 3.0 x 2.0 = 0.6: the intermediates are not capped"
    );
    // A half unit rounds away from zero: 1 ppb x 1.5 = 1.5 ppb -> 2; 1 ppb x 1.499 = 1.499 -> 1.
    assert_eq!(carrier_rate("1.5", "1.0", "0.000000001"), 2);
    assert_eq!(carrier_rate("1.499", "1.0", "0.000000001"), 1);
}

#[test]
fn an_sd_override_gives_the_share_weighted_spread() {
    let paternal = "[paternal.A]\nheight = { sd = 10 }\n";
    let start = "[start.s]\npaternal = { A = 50, other = 50 }\nmaternal = { B = 100 }\n";
    let genetics = mini(
        paternal,
        "[maternal.B]\nheight = { mean = 12 }\n",
        start,
        "",
    );
    let pop = genetics.seed_pop("c").unwrap();
    // Paternal: half 10 and half the default 6 = 8; maternal: the default 6; the pop is their average, 7.
    assert_eq!(pop.sd[0], 7000);
    // Means: paternal all default 10; maternal 12: the average is 11.
    assert_eq!(pop.mean[0], 11_000);
    assert_eq!(pop.sd[1], 6000);
}

// --- blending --------------------------------------------------------------------------------------------------------

fn shares(list: &[(&str, u32)]) -> BTreeMap<String, u32> {
    list.iter()
        .map(|(name, share)| ((*name).to_owned(), *share))
        .collect()
}
fn state(paternal: &[(&str, u32)]) -> PopGenetics {
    PopGenetics {
        paternal: shares(paternal),
        maternal: shares(&[("H", 1000)]),
        ..PopGenetics::default()
    }
}

#[test]
fn blended_share_sets_are_apportioned_to_exactly_a_thousand() {
    // Two receiver heads, then one mover: 200/3 and 2650/3 and 150/3 floor to 66, 883, 50 (999); the largest remainder
    // (I1, 2/3) takes the spare unit.
    let mut receiver = state(&[("R1b", 950), ("I1", 0), ("other", 50)]);
    let mover = state(&[("R1b", 750), ("I1", 200), ("other", 50)]);
    let source = mover.clone();
    receiver.blend(2, &mover, 1).unwrap();
    assert_eq!(
        receiver.paternal,
        shares(&[("I1", 67), ("R1b", 883), ("other", 50)])
    );
    assert_eq!(receiver.paternal.values().sum::<u32>(), 1000);
    assert_eq!(mover, source, "the source is unchanged");
    // An equal mix of two near-thirds ties on the remainders; the sorted key order breaks it.
    let mut a = state(&[("A", 334), ("B", 333), ("C", 333)]);
    a.blend(1, &state(&[("A", 333), ("B", 334), ("C", 333)]), 1)
        .unwrap();
    assert_eq!(a.paternal, shares(&[("A", 334), ("B", 333), ("C", 333)]));
    assert_eq!(a.paternal.values().sum::<u32>(), 1000);
    // A key only the mover has joins the receiver's line.
    let mut b = state(&[("A", 1000)]);
    b.blend(3, &state(&[("B", 1000)]), 1).unwrap();
    assert_eq!(b.paternal, shares(&[("A", 750), ("B", 250)]));
}

#[test]
fn blended_scalars_are_people_weighted_and_round_ties_away_from_zero() {
    let mut receiver = state(&[("A", 1000)]);
    receiver.carrier.insert("c".into(), 100);
    receiver.mean[0] = 1000;
    receiver.mean[1] = -1000;
    let mut mover = state(&[("A", 1000)]);
    mover.carrier.insert("c".into(), 500);
    mover.mean[0] = 1001;
    mover.mean[1] = -1001;
    let mut equal = receiver.clone();
    receiver.blend(3, &mover, 1).unwrap();
    assert_eq!(receiver.carrier["c"], 200, "(3 x 100 + 1 x 500) / 4");
    equal.blend(1, &mover, 1).unwrap();
    assert_eq!(
        (equal.mean[0], equal.mean[1]),
        (1001, -1001),
        "1000.5 rounds away from zero"
    );
}

#[test]
fn an_empty_or_unseeded_receiver_takes_the_movers_state_exactly() {
    let mover = state(&[("A", 333), ("B", 667)]);
    let mut empty = PopGenetics::default();
    empty.blend(40, &mover, 5).unwrap();
    assert_eq!(empty, mover);
    let mut none = state(&[("A", 1000)]);
    none.blend(0, &mover, 5).unwrap();
    assert_eq!(none, mover);
    let mut kept = mover.clone();
    kept.blend(5, &PopGenetics::default(), 5).unwrap();
    assert_eq!(kept, mover, "nothing to blend in from an unseeded pop");
}

#[test]
fn the_pop_state_round_trips_through_the_canonical_form() {
    let genetics = shipped_rules().genetics;
    let pop = genetics.seed_pop("ulaid").unwrap();
    assert_eq!(
        PopGenetics::from_bytes(&pop.to_bytes().unwrap()).unwrap(),
        pop
    );
    assert_eq!(
        PopGenetics::from_bytes(&PopGenetics::default().to_bytes().unwrap()).unwrap(),
        PopGenetics::default()
    );
    assert_eq!(ATTRIBUTES.len(), ATTRIBUTE_COUNT);
}

#[test]
fn a_household_merged_into_a_cohort_blends_its_genetic_state() {
    let rules = shipped_rules();
    let make = |people| {
        crate::make_pop(
            &rules,
            "labourer",
            "ulaid",
            "christianity",
            people,
            &BTreeMap::new(),
        )
        .unwrap()
    };
    let mut pops = vec![make(300)];
    assert_eq!(
        pops[0].genetics,
        rules.genetics.seed_pop("ulaid").unwrap(),
        "seeded from the culture"
    );
    // The same culture gives the same state, so a merge changes nothing.
    crate::merge_into_cohort(&mut pops, make(100)).unwrap();
    assert_eq!(pops[0].genetics, rules.genetics.seed_pop("ulaid").unwrap());
    // A different state blends in by people: 400 heads at 170,000 and 100 at 175,000 give 171,000.
    let mut other = make(100);
    other.genetics.mean[0] = 175_000;
    crate::merge_into_cohort(&mut pops, other).unwrap();
    assert_eq!(pops.len(), 1);
    assert_eq!(pops[0].genetics.mean[0], 171_000);
}

#[test]
fn a_table_named_other_is_refused_because_the_name_is_reserved() {
    for (kind, text) in [
        (
            "paternal",
            "\n[paternal.other.trait_rates]\nprodigious_memory = 20\n",
        ),
        ("maternal", "\n[maternal.other]\nblood = { mean = 9 }\n"),
    ] {
        let haplogroups = format!("{}{text}", haplogroups());
        refused(
            &haplogroups,
            traits(),
            &["ulaid"],
            &format!("{kind}.other"),
            "reserved",
        );
    }
    // The shipped lines keep their meaning: `other` is the default table and the multiplier one.
    let irish = shipped_rules().genetics.seed_pop("ulaid").unwrap();
    assert_eq!(irish.founder["prodigious_memory"], 59_000_000);
}

#[test]
fn a_blend_whose_exact_sum_does_not_fit_is_refused_and_changes_nothing() {
    let mut big = state(&[("A", 1000)]);
    big.mean[0] = i64::MAX;
    let before = big.clone();
    assert_eq!(
        big.blend(u64::MAX, &before.clone(), u64::MAX),
        Err(crate::genetics::BlendOverflow)
    );
    assert_eq!(big, before, "an error leaves the state as it was");
}

#[test]
fn a_cohort_merge_that_overflows_returns_the_named_error_instead_of_panicking() {
    let rules = shipped_rules();
    let make = |people| {
        crate::make_pop(
            &rules,
            "labourer",
            "ulaid",
            "christianity",
            people,
            &BTreeMap::new(),
        )
        .unwrap()
    };
    let mut cohort = make(10);
    cohort.workers = u64::MAX;
    cohort.dependents = 0;
    cohort.genetics.mean[0] = i64::MAX;
    let mut household = make(4);
    household.workers = 4;
    household.dependents = 0;
    household.genetics.mean[0] = i64::MAX;
    let mut pops = vec![cohort];
    let error = crate::merge_into_cohort(&mut pops, household).unwrap_err();
    assert_eq!(
        (error.entry.as_str(), error.problem.as_str()),
        ("start.household", "worker count overflow")
    );
}
