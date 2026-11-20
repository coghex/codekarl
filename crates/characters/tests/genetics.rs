//! Character genetics contracts (CK-361): the arithmetic, the founders, the children and the creation path.
//!
//! Statistical tests use fixed seeds and integer arithmetic; each states its tolerance in the assertion.

use ck_characters::{
    Archive, Character, CharacterStore, Context, Error, GeneticRecord, Lineage, NewCharacter,
    Office, PopLocator, Populations, Sex, Tier, UNKNOWN, character_stream,
    genetics::{normal_draw, succeeds},
};
use ck_economy::{
    Genetics, PopGenetics, Rules,
    genetics::{ATTRIBUTES, PPB, attribute_index},
};
use ck_sim::{Canonical, Date, GameTime};
use std::{path::PathBuf, sync::OnceLock};

fn asset(name: &str) -> &'static str {
    static FILES: OnceLock<std::sync::Mutex<std::collections::BTreeMap<String, &'static str>>> =
        OnceLock::new();
    let mut files = FILES.get_or_init(Default::default).lock().expect("lock");
    files.entry(name.to_owned()).or_insert_with(|| {
        // Read at run time, not with `include_str!`, so a crate built without the assets directory still compiles.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
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

fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
}
fn pop() -> PopLocator {
    PopLocator {
        place: "a".into(),
        slot: 0,
    }
}
fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .expect("shipped rules")
    })
}
/// The shipped rules, or the shipped rules plus extra trait text.
fn fixture(extra: &str) -> Genetics {
    Genetics::parse(
        ("haplogroups.toml", haplogroups()),
        ("traits.toml", &format!("{}\n{extra}", traits())),
        &[],
    )
    .expect("fixture rules")
}
fn context(genetics: &Genetics, seed: u64) -> Context<'_> {
    Context {
        rules: genetics,
        personality: &rules().personality,
        seed,
        tick: date(768, 1, 1),
    }
}
fn irish(genetics: &Genetics) -> PopGenetics {
    genetics.seed_pop("ulaid").expect("Irish start set")
}
/// Every pop is Irish (`ulaid`); the head count comes from the wrapped function.
struct Pops<F>(F);
impl<F: Fn(&PopLocator) -> Option<u64>> Populations for Pops<F> {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        (self.0)(locator)
    }
    fn culture(&self, _: &PopLocator) -> Option<String> {
        Some("ulaid".to_owned())
    }
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "the closure type `Pops` wraps is `Fn(&PopLocator) -> Option<u64>`"
)]
fn roomy(_: &PopLocator) -> Option<u64> {
    Some(1_000_000)
}
fn new(sex: Sex) -> NewCharacter {
    NewCharacter {
        name: "x".into(),
        sex,
        birth: date(768, 1, 1),
        pop: pop(),
    }
}
fn founder(store: &mut CharacterStore, ctx: &Context<'_>, sex: Sex, state: &PopGenetics) -> u64 {
    store
        .create_founder(ctx, new(sex), state, &Pops(roomy))
        .expect("founder")
}
fn child(
    store: &mut CharacterStore,
    ctx: &Context<'_>,
    sex: Sex,
    parents: (u64, u64),
    state: &PopGenetics,
    archive: &Archive,
) -> Result<u64, Error> {
    store.create_child(
        ctx,
        new(sex),
        state,
        &Lineage {
            father: parents.0,
            mother: parents.1,
            archive,
        },
        &Pops(roomy),
    )
}
fn archive(tag: &str) -> Archive {
    let path: PathBuf =
        std::env::temp_dir().join(format!("ck-genetics-{}-{tag}", std::process::id()));
    std::fs::remove_file(&path).ok();
    Archive::open(path).expect("archive")
}
fn record(store: &CharacterStore, id: u64) -> &GeneticRecord {
    &store.get(id).expect("live").genetics
}
/// Per-mille of `count` in `total`.
fn permille(count: usize, total: usize) -> u64 {
    u64::try_from(count * 1000 / total).expect("fits")
}
/// A fixed, deterministic pick source.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33) % u64::try_from(n).expect("fits")).expect("fits")
    }
}
/// The sampling tolerance of a proportion `p` (per mille) over `n` draws: five standard errors plus 3 per mille.
fn tolerance(p: u64, n: usize) -> u64 {
    5 * (p * (1000 - p) / u64::try_from(n).expect("fits")).isqrt() + 3
}
fn within(actual: u64, expected: u64, tolerance: u64, what: &str) {
    assert!(
        actual.abs_diff(expected) <= tolerance,
        "{what}: {actual} per mille, expected {expected} +- {tolerance}"
    );
}

// --- arithmetic ------------------------------------------------------------------------------------------------------

#[test]
fn the_normal_draw_matches_pinned_vectors_computed_independently() {
    // Computed outside this program from the stream's frozen definition: twelve uniform draws below 1,000,000, summed
    // and centred, scaled by the spread and rounded to the nearest thousandth.
    let tick = GameTime::from_hours(1000);
    let mut stream = character_stream(42, 7, "value.height", tick);
    assert_eq!(normal_draw(&mut stream, 170_000, 6_000), Ok(174_280));
    let mut stream = character_stream(42, 7, "value.blood", tick);
    assert_eq!(normal_draw(&mut stream, 5_475, 2_000), Ok(7_071));
    let mut stream = character_stream(9, 1, "value.wit", GameTime::from_hours(24_000));
    assert_eq!(normal_draw(&mut stream, 10_000, 2_000), Ok(9_686));
    // A tie in the whole value: 170,000 + 6,000 x (-1,877,750) / 1,000,000 is exactly 158,733.5, which rounds away from
    // zero to 158,734 (rounding the offset alone first would give 158,733).
    let mut stream = character_stream(19, 7, "value.height", tick);
    assert_eq!(normal_draw(&mut stream, 170_000, 6_000), Ok(158_734));
}

#[test]
fn a_chance_succeeds_below_its_threshold_and_fails_at_it() {
    for p in [1, 100_000, 500_000, PPB - 1] {
        assert!(succeeds(u64::from(p) - 1, p), "draw p-1 succeeds at {p}");
        assert!(!succeeds(u64::from(p), p), "draw p fails at {p}");
    }
    assert!(!succeeds(0, 0), "a zero chance never succeeds");
    assert!(
        succeeds(u64::from(PPB) - 1, PPB),
        "a certain chance always does"
    );
}

// --- founders --------------------------------------------------------------------------------------------------------

#[test]
fn a_founder_takes_haplogroups_baseline_and_values_from_its_pop() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 11);
    let mut store = CharacterStore::default();
    let mut others = 0;
    let mut height = 0_i128;
    let samples = 600;
    for _ in 0..samples {
        let id = founder(&mut store, &ctx, Sex::Male, &state);
        let record = record(&store, id);
        assert!(state.paternal.contains_key(&record.paternal));
        assert!(state.maternal.contains_key(&record.maternal));
        assert_eq!((record.mean, record.sd), (state.mean, state.sd));
        others += usize::from(record.paternal == "other");
        height += i128::from(record.value[0]);
    }
    // Irish paternal shares are R1b 950 and other 50.
    within(
        permille(others, samples),
        50,
        tolerance(50, samples),
        "other lineage",
    );
    let average = height / i128::try_from(samples).expect("fits");
    assert!(
        (average - 170_400_i128 + 400).abs() < 1_000,
        "founder height averages the pop's mean: {average}"
    );
}

#[test]
fn a_woman_shows_the_offset_but_it_never_enters_a_baseline_or_a_genetic_value() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 12);
    let mut store = CharacterStore::default();
    let samples = 500;
    let (mut male_total, mut female_total) = (0_i128, 0_i128);
    for _ in 0..samples {
        let son = founder(&mut store, &ctx, Sex::Male, &state);
        let daughter = founder(&mut store, &ctx, Sex::Female, &state);
        male_total += i128::from(record(&store, son).value[0]);
        let woman_record = record(&store, daughter);
        female_total += i128::from(woman_record.value[0]);
        assert_eq!(woman_record.mean, state.mean, "no offset in a baseline");
        assert_eq!(
            woman_record.shown(genetics, Sex::Female, 0),
            (woman_record.value[0] - 12_000).max(100_000)
        );
        assert_eq!(
            woman_record.shown(genetics, Sex::Male, 0),
            woman_record.value[0]
        );
        assert_eq!(
            woman_record.shown(genetics, Sex::Female, 1),
            (woman_record.value[1] - 10_000).max(30_000)
        );
        assert_eq!(
            woman_record.shown(genetics, Sex::Female, 2),
            (woman_record.value[2] - 2_000).max(1_000)
        );
    }
    let gap = (male_total - female_total) / i128::from(samples);
    assert!(
        gap.abs() < 1_500,
        "genetic values do not differ by sex: {gap}"
    );
}

#[test]
fn values_are_clamped_at_birth_and_baselines_never_are() {
    let genetics = &rules().genetics;
    let mut state = irish(genetics);
    let blood = attribute_index("blood").expect("humour");
    let strength = attribute_index("strength").expect("stat");
    let (height, weight) = (0, 1);
    state.mean[blood] = 10_000;
    state.sd[blood] = 20_000;
    state.mean[strength] = 500; // below the floor of 1: the baseline stays
    state.sd[strength] = 30_000;
    state.mean[height] = 100_000;
    state.sd[height] = 30_000;
    state.mean[weight] = 30_000;
    state.sd[weight] = 20_000;
    let ctx = context(genetics, 13);
    let mut store = CharacterStore::default();
    let mut seen = [(i64::MAX, i64::MIN); 4];
    for _ in 0..800 {
        let id = founder(&mut store, &ctx, Sex::Male, &state);
        let record = record(&store, id);
        assert_eq!(record.mean[strength], 500, "a baseline is not clamped");
        for (slot, attribute) in [blood, strength, height, weight].into_iter().enumerate() {
            let value = record.value[attribute];
            seen[slot] = (seen[slot].0.min(value), seen[slot].1.max(value));
        }
    }
    assert_eq!(seen[0], (0, 10_000), "humours are held at both ends");
    assert_eq!(seen[1].0, 1_000, "a stat is floored at 1");
    assert!(seen[1].1 > 20_000, "and has no cap");
    assert_eq!(seen[2].0, 100_000, "height is floored at 100 cm");
    assert_eq!(seen[3].0, 30_000, "weight is floored at 30 kg");
}

#[test]
fn founders_follow_the_pops_carrier_and_founder_rates() {
    let genetics = &rules().genetics;
    let mut state = irish(genetics);
    let ctx = context(genetics, 14);
    // Prodigious memory's seeded founder rate for an Irish pop is 59,000,000 ppb.
    assert_eq!(state.founder["prodigious_memory"], 59_000_000);
    let mut store = CharacterStore::default();
    let samples = 8000;
    let mut with = 0;
    for _ in 0..samples {
        let id = founder(&mut store, &ctx, Sex::Female, &state);
        with += usize::from(record(&store, id).level["prodigious_memory"] > 0);
    }
    within(
        permille(with, samples),
        59,
        tolerance(59, samples),
        "prodigious memory",
    );
    // A founder's own haplogroup is not read again: the rate is the pop's, so changing it changes the result.
    state.founder.insert("prodigious_memory".into(), 0);
    let id = founder(&mut store, &ctx, Sex::Female, &state);
    assert_eq!(record(&store, id).level["prodigious_memory"], 0);
    // Graded founder levels follow founder_q: with a certain trait each level is 0.45 times the one below.
    state.founder.insert("beauty".into(), PPB);
    let mut counts = [0_usize; 7];
    for _ in 0..4000 {
        let id = founder(&mut store, &ctx, Sex::Female, &state);
        counts[usize::from(record(&store, id).level["beauty"])] += 1;
    }
    assert_eq!(counts[0], 0, "a certain trait always shows a level");
    // Weights 1 : 0.45 : 0.2025 ... sum to 1.80308: level 1 has about 55.5% and level 2 about 25.0%.
    within(
        permille(counts[1], 4000),
        555,
        tolerance(555, 4000),
        "level 1",
    );
    within(
        permille(counts[2], 4000),
        250,
        tolerance(250, 4000),
        "level 2",
    );
}

// --- children --------------------------------------------------------------------------------------------------------

#[test]
fn children_regress_towards_the_mean_by_one_minus_heritability() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 21);
    let mut store = CharacterStore::default();
    let none = archive("regress");
    let mut best = [(i64::MIN, 0_u64), (i64::MIN, 0_u64)];
    for _ in 0..300 {
        for (slot, sex) in [Sex::Male, Sex::Female].into_iter().enumerate() {
            let id = founder(&mut store, &ctx, sex, &state);
            let value = record(&store, id).value[0];
            if value > best[slot].0 {
                best[slot] = (value, id);
            }
        }
    }
    let (father, mother) = (best[0].1, best[1].1);
    let samples = 1500_i128;
    let mut totals = [0_i128; 9];
    for index in 0..samples {
        let sex = if index % 2 == 0 {
            Sex::Male
        } else {
            Sex::Female
        };
        let id = child(&mut store, &ctx, sex, (father, mother), &state, &none).expect("child");
        for (attribute, total) in totals.iter_mut().enumerate() {
            *total += i128::from(record(&store, id).value[attribute]);
        }
    }
    for (attribute, name) in ATTRIBUTES.iter().enumerate().take(9) {
        let (f, m) = (record(&store, father), record(&store, mother));
        let mean_b = i128::from(state.mean[attribute]);
        let middle = i128::from(f.value[attribute] + m.value[attribute]);
        let h = i128::from(genetics.heritability(attribute));
        let expected = mean_b + h * (middle - 2 * mean_b) / 2000;
        let measured = totals[attribute] / samples;
        let spread =
            i128::from(state.sd[attribute]) * i128::from(genetics.spread(attribute)) / 1000;
        // Five standard errors of the mean of 1500 draws.
        let tolerance = 5 * spread * 1000 / 38_730;
        assert!(
            (measured - expected).abs() <= tolerance + 5,
            "{name}: children average {measured}, expected {expected} +- {tolerance}"
        );
    }
    // The pull towards the mean for height (h = 0.8): the children keep about 80% of the parents' deviation.
    let middle = i128::from(record(&store, father).value[0] + record(&store, mother).value[0]);
    let deviation = middle / 2 - i128::from(state.mean[0]);
    assert!(
        deviation > 6_000,
        "the chosen parents are well above the mean: {deviation}"
    );
    let kept = (totals[0] / samples - i128::from(state.mean[0])) * 1000 / deviation;
    assert!(
        (720..=880).contains(&kept),
        "height keeps {kept} per mille of the deviation"
    );
}

#[test]
fn a_closed_population_keeps_each_stats_spread_over_ten_generations() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 22);
    let mut store = CharacterStore::default();
    let none = archive("spread");
    let mut people: Vec<(u64, Sex)> = Vec::new();
    for _ in 0..200 {
        people.push((founder(&mut store, &ctx, Sex::Male, &state), Sex::Male));
        people.push((founder(&mut store, &ctx, Sex::Female, &state), Sex::Female));
    }
    let mut pick = Lcg(5);
    for _ in 0..10 {
        let men: Vec<u64> = people
            .iter()
            .filter(|p| p.1 == Sex::Male)
            .map(|p| p.0)
            .collect();
        let women: Vec<u64> = people
            .iter()
            .filter(|p| p.1 == Sex::Female)
            .map(|p| p.0)
            .collect();
        let mut next = Vec::new();
        for index in 0..400 {
            let sex = if index % 2 == 0 {
                Sex::Male
            } else {
                Sex::Female
            };
            let parents = (men[pick.next(men.len())], women[pick.next(women.len())]);
            let id = child(&mut store, &ctx, sex, parents, &state, &none).expect("child");
            next.push((id, sex));
        }
        people = next;
    }
    // Every stat and every humour (the humours are held to 0..10, which trims their tails a little).
    for (attribute, name) in ATTRIBUTES.iter().enumerate() {
        let values: Vec<i128> = people
            .iter()
            .map(|p| i128::from(record(&store, p.0).value[attribute]))
            .collect();
        let count = i128::try_from(values.len()).expect("fits");
        let mean = values.iter().sum::<i128>() / count;
        let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<i128>() / count;
        let sd = i128::from(state.sd[attribute]);
        // The variance ratio to the stat's sd squared: height near 6 cm (not near 1.5), within 25% (sd within about 12%).
        let ratio = variance * 100 / (sd * sd);
        assert!(
            (78..=125).contains(&ratio),
            "{name}: spread variance ratio {ratio}% after ten generations"
        );
    }
}

#[test]
fn a_childs_baseline_is_the_parents_average() {
    let genetics = &rules().genetics;
    let ctx = context(genetics, 23);
    let mut store = CharacterStore::default();
    let none = archive("baseline");
    let mut a = irish(genetics);
    a.mean[3] = 11_001;
    a.sd[3] = 2_001;
    let mut b = irish(genetics);
    b.mean[3] = 12_000;
    b.sd[3] = 2_000;
    let father = founder(&mut store, &ctx, Sex::Male, &a);
    let mother = founder(&mut store, &ctx, Sex::Female, &b);
    let id = child(&mut store, &ctx, Sex::Female, (father, mother), &a, &none).expect("child");
    // 11,001 and 12,000 average to 11,500.5, which rounds away from zero; 2,001 and 2,000 likewise.
    assert_eq!(record(&store, id).mean[3], 11_501);
    assert_eq!(record(&store, id).sd[3], 2_001);
}

// --- Mendelian conditions --------------------------------------------------------------------------------------------

fn carriers(genetics: &Genetics, conditions: &[&str]) -> PopGenetics {
    let mut state = irish(genetics);
    for name in conditions {
        state.carrier.insert((*name).into(), PPB / 2);
    }
    state
}
fn copies(store: &CharacterStore, id: u64, name: &str) -> u8 {
    record(store, id).mendelian[name].copies
}
fn find(
    store: &mut CharacterStore,
    ctx: &Context<'_>,
    sex: Sex,
    state: &PopGenetics,
    name: &str,
    wanted: u8,
) -> u64 {
    for _ in 0..2000 {
        let id = founder(store, ctx, sex, state);
        if copies(store, id, name) == wanted {
            return id;
        }
    }
    panic!("no founder with {wanted} copies of {name}");
}

#[test]
fn an_ordinary_recessive_gives_a_quarter_affected_half_carriers_and_a_quarter_clear() {
    let genetics = &rules().genetics;
    let state = carriers(genetics, &["albino"]);
    let ctx = context(genetics, 31);
    let mut store = CharacterStore::default();
    let none = archive("ordinary");
    let father = find(&mut store, &ctx, Sex::Male, &state, "albino", 1);
    let mother = find(&mut store, &ctx, Sex::Female, &state, "albino", 1);
    let samples = 4000;
    let mut counts = [0_usize; 3];
    for index in 0..samples {
        let sex = if index % 2 == 0 {
            Sex::Male
        } else {
            Sex::Female
        };
        let id = child(&mut store, &ctx, sex, (father, mother), &state, &none).expect("child");
        let albino = record(&store, id).mendelian["albino"];
        counts[usize::from(albino.copies)] += 1;
        assert_eq!(
            albino.shows,
            albino.copies == 2,
            "penetrance 1: two copies show"
        );
    }
    within(
        permille(counts[2], samples),
        250,
        tolerance(250, samples),
        "affected",
    );
    within(
        permille(counts[1], samples),
        500,
        tolerance(500, samples),
        "carriers",
    );
    within(
        permille(counts[0], samples),
        250,
        tolerance(250, samples),
        "clear",
    );
}

#[test]
fn an_x_recessive_carrier_mother_gives_half_her_sons_the_condition_and_no_daughter() {
    let genetics = &rules().genetics;
    let state = carriers(genetics, &["bleeder"]);
    let ctx = context(genetics, 32);
    let mut store = CharacterStore::default();
    let none = archive("x");
    let mother = find(&mut store, &ctx, Sex::Female, &state, "bleeder", 1);
    let father = find(&mut store, &ctx, Sex::Male, &state, "bleeder", 0);
    let (mut sons, mut affected_sons, mut daughters, mut carrier_daughters) = (0, 0, 0, 0);
    for index in 0..4000 {
        let sex = if index % 2 == 0 {
            Sex::Male
        } else {
            Sex::Female
        };
        let id = child(&mut store, &ctx, sex, (father, mother), &state, &none).expect("child");
        let bleeder = record(&store, id).mendelian["bleeder"];
        if sex == Sex::Male {
            sons += 1;
            affected_sons += usize::from(bleeder.shows);
            assert_eq!(bleeder.shows, bleeder.copies == 1, "a man's one copy shows");
        } else {
            daughters += 1;
            carrier_daughters += usize::from(bleeder.copies == 1);
            assert!(!bleeder.shows, "no affected daughter");
            assert!(bleeder.copies <= 1);
        }
    }
    within(
        permille(affected_sons, sons),
        500,
        tolerance(500, sons),
        "affected sons",
    );
    within(
        permille(carrier_daughters, daughters),
        500,
        tolerance(500, daughters),
        "carrier daughters",
    );
}

#[test]
fn a_male_founder_has_one_x_copy_and_a_female_founder_two_and_no_y() {
    let extra = "[mendelian.y_mark]\nmode = \"y\"\npenetrance = 1.0\nallele = 1.0\n";
    let genetics = fixture(extra);
    let mut state = irish(&genetics);
    state.carrier.insert("bleeder".into(), PPB);
    assert_eq!(state.carrier["y_mark"], PPB);
    let ctx = context(&genetics, 33);
    let mut store = CharacterStore::default();
    let man = founder(&mut store, &ctx, Sex::Male, &state);
    let woman = founder(&mut store, &ctx, Sex::Female, &state);
    assert_eq!(
        (
            copies(&store, man, "bleeder"),
            copies(&store, woman, "bleeder")
        ),
        (1, 2)
    );
    assert_eq!(
        (
            copies(&store, man, "y_mark"),
            copies(&store, woman, "y_mark")
        ),
        (1, 0)
    );
    // A certain maternal condition is one copy in anyone.
    state.carrier.insert("twin_bearer".into(), PPB);
    let (man, woman) = (
        founder(&mut store, &ctx, Sex::Male, &state),
        founder(&mut store, &ctx, Sex::Female, &state),
    );
    assert_eq!(
        (
            copies(&store, man, "twin_bearer"),
            copies(&store, woman, "twin_bearer")
        ),
        (1, 1)
    );
}

#[test]
fn the_y_line_passes_from_fathers_to_every_son_and_the_maternal_line_from_mothers_to_all() {
    let extra = "[mendelian.y_mark]\nmode = \"y\"\npenetrance = 1.0\nallele = 0.5\n";
    let genetics = fixture(extra);
    let mut state = irish(&genetics);
    state.carrier.insert("twin_bearer".into(), PPB / 2);
    let ctx = context(&genetics, 34);
    let mut store = CharacterStore::default();
    let none = archive("lineage");
    let mut men: Vec<u64> = Vec::new();
    let mut women: Vec<u64> = Vec::new();
    for _ in 0..60 {
        men.push(founder(&mut store, &ctx, Sex::Male, &state));
        women.push(founder(&mut store, &ctx, Sex::Female, &state));
    }
    let mut pick = Lcg(9);
    for _ in 0..4 {
        let (mut next_men, mut next_women) = (Vec::new(), Vec::new());
        for index in 0..120 {
            let sex = if index % 2 == 0 {
                Sex::Male
            } else {
                Sex::Female
            };
            let (father, mother) = (men[pick.next(men.len())], women[pick.next(women.len())]);
            let id = child(&mut store, &ctx, sex, (father, mother), &state, &none).expect("child");
            let (f, m, c) = (
                record(&store, father),
                record(&store, mother),
                record(&store, id),
            );
            assert_eq!(c.paternal, f.paternal, "everyone records the father's line");
            assert_eq!(
                c.maternal, m.maternal,
                "everyone gets the mother's mitochondria"
            );
            let y = c.mendelian["y_mark"].copies;
            assert_eq!(
                y,
                if sex == Sex::Male {
                    f.mendelian["y_mark"].copies
                } else {
                    0
                }
            );
            let twin = c.mendelian["twin_bearer"];
            assert_eq!(
                twin.copies, m.mendelian["twin_bearer"].copies,
                "the mother's, whatever the father has"
            );
            assert_eq!(
                twin.shows,
                twin.copies == 1 && sex == Sex::Female,
                "shown by women only"
            );
            if sex == Sex::Male {
                next_men.push(id);
            } else {
                next_women.push(id);
            }
        }
        (men, women) = (next_men, next_women);
    }
}

#[test]
fn expression_follows_penetrance_and_the_sex_limit_for_founders() {
    let extra = "\
[mendelian.certain]\nmode = \"ordinary\"\ndominance = \"dominant\"\npenetrance = 1.0\nallele = 1.0\n\
[mendelian.never]\nmode = \"ordinary\"\ndominance = \"dominant\"\npenetrance = 0.0\nallele = 1.0\n\
[mendelian.hidden_in_men]\nmode = \"ordinary\"\ndominance = \"dominant\"\npenetrance = 1.0\nallele = 1.0\nshows_in = \"women\"\n\
[mendelian.recessive_clear]\nmode = \"ordinary\"\ndominance = \"recessive\"\npenetrance = 1.0\nallele = 0.0\n";
    let genetics = fixture(extra);
    let state = irish(&genetics);
    let ctx = context(&genetics, 35);
    let mut store = CharacterStore::default();
    for sex in [Sex::Male, Sex::Female] {
        let id = founder(&mut store, &ctx, sex, &state);
        let r = record(&store, id);
        assert!(r.mendelian["certain"].shows);
        assert!(!r.mendelian["never"].shows);
        assert_eq!(
            r.mendelian["hidden_in_men"].copies, 2,
            "both sexes carry it"
        );
        assert_eq!(r.mendelian["hidden_in_men"].shows, sex == Sex::Female);
        assert!(!r.mendelian["recessive_clear"].shows);
    }
}

#[test]
fn deaf_and_blind_are_weak_dominants_that_show_about_a_tenth_of_the_time() {
    let genetics = &rules().genetics;
    let mut state = irish(genetics);
    state.carrier.insert("deaf".into(), PPB);
    state.carrier.insert("blind".into(), PPB);
    let ctx = context(genetics, 36);
    let mut store = CharacterStore::default();
    let samples = 4000;
    let (mut deaf, mut blind) = (0, 0);
    for _ in 0..samples {
        let id = founder(&mut store, &ctx, Sex::Male, &state);
        let r = record(&store, id);
        assert_eq!(
            (r.mendelian["deaf"].copies, r.mendelian["blind"].copies),
            (2, 2)
        );
        deaf += usize::from(r.mendelian["deaf"].shows);
        blind += usize::from(r.mendelian["blind"].shows);
    }
    within(
        permille(deaf, samples),
        100,
        tolerance(100, samples),
        "deaf",
    );
    within(
        permille(blind, samples),
        100,
        tolerance(100, samples),
        "blind",
    );
}

// --- graded traits ---------------------------------------------------------------------------------------------------

fn leveled(
    store: &mut CharacterStore,
    ctx: &Context<'_>,
    sex: Sex,
    base: &PopGenetics,
    level: u8,
) -> u64 {
    let mut state = base.clone();
    state
        .founder
        .insert("beauty".into(), if level == 0 { 0 } else { PPB });
    for _ in 0..5000 {
        let id = founder(store, ctx, sex, &state);
        if record(store, id).level["beauty"] == level {
            return id;
        }
    }
    panic!("no founder of level {level}");
}

/// A row of inheritance-math-r1's exact table (percent x 10, levels none..SSS): the children's distribution.
fn graded_row(father: u8, mother: u8, expected: [u64; 7], seed: u64) {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, seed);
    let mut store = CharacterStore::default();
    let none = archive(&format!("graded-{father}-{mother}-{seed}"));
    let dad = leveled(&mut store, &ctx, Sex::Male, &state, father);
    let mum = leveled(&mut store, &ctx, Sex::Female, &state, mother);
    let samples = 6000;
    let mut counts = [0_usize; 7];
    for index in 0..samples {
        let sex = if index % 2 == 0 {
            Sex::Male
        } else {
            Sex::Female
        };
        let id = child(&mut store, &ctx, sex, (dad, mum), &state, &none).expect("child");
        counts[usize::from(record(&store, id).level["beauty"])] += 1;
    }
    for (level, wanted) in expected.into_iter().enumerate() {
        within(
            permille(counts[level], samples),
            wanted,
            tolerance(wanted, samples),
            &format!("father {father} mother {mother} level {level}"),
        );
    }
}

#[test]
fn graded_children_follow_the_designed_distribution_none_by_none() {
    graded_row(0, 0, [989, 8, 2, 1, 0, 0, 0], 41);
}
#[test]
fn graded_children_follow_the_designed_distribution_c_by_none() {
    graded_row(1, 0, [740, 247, 12, 1, 0, 0, 0], 42);
}
#[test]
fn graded_children_follow_the_designed_distribution_a_by_s() {
    graded_row(3, 4, [0, 4, 155, 589, 243, 10, 0], 43);
}
#[test]
fn graded_children_follow_the_designed_distribution_sss_by_none() {
    graded_row(6, 0, [157, 142, 181, 185, 151, 99, 84], 44);
}

// --- the creation path -----------------------------------------------------------------------------------------------

#[test]
fn a_child_of_a_live_ruler_and_mother_is_linked_both_ways_and_full_at_once() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 51);
    let mut store = CharacterStore::default();
    let none = archive("ruler");
    let father = founder(&mut store, &ctx, Sex::Male, &state);
    let mother = founder(&mut store, &ctx, Sex::Female, &state);
    store
        .set_office(father, Some(Office::Ruler), date(768, 1, 1))
        .expect("office");
    let id = child(
        &mut store,
        &ctx,
        Sex::Female,
        (father, mother),
        &state,
        &none,
    )
    .expect("child");
    let record = store.get(id).expect("live");
    assert_eq!((record.father, record.mother), (father, mother));
    assert_eq!(store.get(father).expect("live").children, [id]);
    assert_eq!(store.get(mother).expect("live").children, [id]);
    assert_eq!(store.tier(id, date(768, 1, 1)), Some(Tier::Full));
    assert_eq!(store.tier(mother, date(900, 1, 1)), Some(Tier::Coarse));
    assert_eq!(id, 3, "identifiers are assigned in creation order");
}

#[test]
fn a_child_of_an_archived_parent_links_to_it_and_the_archive_is_unchanged() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 52);
    let mut store = CharacterStore::default();
    let mut dead = archive("archived-parent");
    let father = founder(&mut store, &ctx, Sex::Male, &state);
    let mother = founder(&mut store, &ctx, Sex::Female, &state);
    let before = store.get(father).expect("live").clone();
    store.die(father, date(768, 6, 1), &mut dead).expect("dies");
    let id = child(&mut store, &ctx, Sex::Male, (father, mother), &state, &dead).expect("child");
    assert_eq!(store.get(id).expect("live").father, father);
    assert_eq!(store.get(mother).expect("live").children, [id]);
    let archived = dead.get(father).expect("read").expect("archived");
    assert_eq!(
        archived.children, before.children,
        "the archive is append-only"
    );
    assert_eq!(archived.genetics, before.genetics);
    assert_eq!(
        store
            .lookup(father, &dead)
            .expect("lookup")
            .expect("resolves")
            .id,
        father
    );
}

#[test]
fn a_refused_creation_changes_nothing() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 53);
    let mut store = CharacterStore::default();
    let none = archive("refusals");
    let man = founder(&mut store, &ctx, Sex::Male, &state);
    let woman = founder(&mut store, &ctx, Sex::Female, &state);
    let before = store.clone();
    let bytes = store.to_bytes().expect("encode");
    for (parents, error) in [
        ((woman, woman), Error::WrongSex),
        ((man, man), Error::WrongSex),
        ((woman, man), Error::WrongSex),
        ((999, woman), Error::MissingCharacter),
        ((man, 999), Error::MissingCharacter),
    ] {
        assert_eq!(
            child(&mut store, &ctx, Sex::Male, parents, &state, &none),
            Err(error)
        );
        assert_eq!(store, before, "no id, record or link for {parents:?}");
        assert_eq!(store.to_bytes().expect("encode"), bytes);
    }
    let lineage = Lineage {
        father: man,
        mother: woman,
        archive: &none,
    };
    for heads in [None, Some(2)] {
        let result = store.create_child(
            &ctx,
            new(Sex::Male),
            &state,
            &lineage,
            &Pops(|_: &PopLocator| heads),
        );
        assert_eq!(
            result,
            Err(if heads.is_none() {
                Error::MissingPop
            } else {
                Error::PopFull
            })
        );
        assert_eq!(store, before);
    }
    let result = store.create_founder(
        &ctx,
        new(Sex::Male),
        &state,
        &Pops(|_: &PopLocator| Some(2)),
    );
    assert_eq!(result, Err(Error::PopFull));
    assert_eq!(
        store.create_founder(&ctx, new(Sex::Male), &PopGenetics::default(), &Pops(roomy)),
        Err(Error::Genetics),
        "an unseeded pop cannot make genetics"
    );
    assert_eq!(store, before);
    // The next creation still gets the next identifier.
    assert_eq!(
        child(&mut store, &ctx, Sex::Male, (man, woman), &state, &none),
        Ok(3)
    );
}

#[test]
fn an_unknown_parent_is_a_synthetic_founder_of_the_childs_pop_and_is_not_stored() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 54);
    let mut store = CharacterStore::default();
    let none = archive("unknown");
    let mother = founder(&mut store, &ctx, Sex::Female, &state);
    let id = child(
        &mut store,
        &ctx,
        Sex::Male,
        (UNKNOWN, mother),
        &state,
        &none,
    )
    .expect("child");
    let record = store.get(id).expect("live");
    assert_eq!((record.father, record.mother), (UNKNOWN, mother));
    assert!(state.paternal.contains_key(&record.genetics.paternal));
    assert_eq!(
        store.iter().count(),
        2,
        "the synthetic parent is not stored"
    );
    assert_eq!(store.get(mother).expect("live").children, [id]);
    // Both parents unknown, through the normal entry point; and the result is reproducible.
    let both = child(
        &mut store,
        &ctx,
        Sex::Female,
        (UNKNOWN, UNKNOWN),
        &state,
        &none,
    )
    .expect("child");
    let mut again = CharacterStore::default();
    let first = founder(&mut again, &ctx, Sex::Female, &state);
    assert_eq!(first, mother);
    assert_eq!(
        child(&mut again, &ctx, Sex::Male, (UNKNOWN, first), &state, &none),
        Ok(id)
    );
    assert_eq!(
        again.get(id).expect("live").genetics,
        store.get(id).expect("live").genetics
    );
    assert!(store.get(both).is_some());
}

#[test]
fn the_genetic_record_survives_save_load_death_and_set_parents() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 55);
    let mut store = CharacterStore::default();
    let mut dead = archive("roundtrip");
    let father = founder(&mut store, &ctx, Sex::Male, &state);
    let mother = founder(&mut store, &ctx, Sex::Female, &state);
    let id = child(
        &mut store,
        &ctx,
        Sex::Female,
        (father, mother),
        &state,
        &dead,
    )
    .expect("child");
    let genes = record(&store, id).clone();
    let loaded = CharacterStore::from_bytes(&store.to_bytes().expect("encode")).expect("decode");
    assert_eq!(loaded, store);
    assert_eq!(record(&loaded, id), &genes);
    // Re-pointing the relationship later does not touch genetics.
    store
        .set_parents(id, mother, father, date(768, 2, 1))
        .expect("parents");
    assert_eq!(record(&store, id), &genes);
    // Death archives the record with its genetics.
    store.die(id, date(769, 1, 1), &mut dead).expect("dies");
    let archived: Character = dead.get(id).expect("read").expect("archived");
    assert_eq!(*archived.genetics, genes);
    let reopened = Archive::open(
        std::env::temp_dir().join(format!("ck-genetics-{}-roundtrip", std::process::id())),
    )
    .expect("reopen");
    assert_eq!(
        *reopened.get(id).expect("read").expect("archived").genetics,
        genes
    );
}

#[test]
fn creations_are_deterministic_and_a_later_creation_leaves_earlier_ones_alone() {
    let genetics = &rules().genetics;
    let state = irish(genetics);
    let ctx = context(genetics, 56);
    let make = |extra: bool| {
        let mut store = CharacterStore::default();
        let none = archive("determinism");
        let father = founder(&mut store, &ctx, Sex::Male, &state);
        let mother = founder(&mut store, &ctx, Sex::Female, &state);
        let first =
            child(&mut store, &ctx, Sex::Male, (father, mother), &state, &none).expect("child");
        if extra {
            child(
                &mut store,
                &ctx,
                Sex::Female,
                (mother, father),
                &state,
                &none,
            )
            .expect_err("a woman is not a father");
            child(
                &mut store,
                &ctx,
                Sex::Female,
                (father, mother),
                &state,
                &none,
            )
            .expect("appended");
        }
        (store, first)
    };
    let (plain, a) = make(false);
    let (again, _) = make(false);
    let (appended, b) = make(true);
    assert_eq!(
        plain, again,
        "the same sequence and seed give the same genetics"
    );
    assert_eq!(a, b);
    assert_eq!(
        record(&plain, a),
        record(&appended, b),
        "an appended creation leaves A unchanged"
    );
    let other = context(genetics, 57);
    let mut elsewhere = CharacterStore::default();
    let id = founder(&mut elsewhere, &other, Sex::Male, &state);
    assert_ne!(
        record(&elsewhere, id),
        record(&plain, 1),
        "a different seed gives different genetics"
    );
}

#[test]
fn no_floating_point_type_appears_in_the_genetic_state_or_its_arithmetic() {
    let float32 = concat!("f", "32");
    let float64 = concat!("f", "64");
    for (name, source) in [
        ("characters genetics", include_str!("../src/genetics.rs")),
        (
            "economy genetics",
            include_str!("../../economy/src/genetics.rs"),
        ),
    ] {
        assert!(!source.contains(float32), "{name} uses a 32-bit float");
        assert!(!source.contains(float64), "{name} uses a 64-bit float");
    }
}
