//! CK-359 (characters P1): the noble pop, the demesne and the rent on owned living space.

use std::{collections::BTreeMap, path::Path};

use ck_sim::{Canonical, Date, days_in_month};

use super::*;

fn rules() -> Rules {
    Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
        .unwrap()
}

fn good(rules: &Rules, id: &str) -> usize {
    rules.goods.iter().position(|item| item.id == id).unwrap()
}

fn building<'a>(rules: &'a Rules, id: &str) -> &'a BuildingType {
    rules.buildings.iter().find(|item| item.id == id).unwrap()
}

// ---------------------------------------------------------------- the shipped rules

#[test]
fn the_noble_pop_type_loads_at_exactly_the_approved_values() {
    let rules = rules();
    let noble = rules
        .pop_types
        .iter()
        .find(|pop| pop.id == "noble")
        .expect("the noble pop type");
    assert_eq!(noble.dependent_pm, 400);
    assert_eq!(noble.births_pm_year, 67);
    assert_eq!(noble.worker_deaths_pm_year, 20);
    assert_eq!(noble.dependent_deaths_pm_year, 60);
    assert_eq!(noble.necessities_expectation_pm, 1000);
    assert_eq!(noble.niceties_expectation_pm, 700);
    assert_eq!(noble.luxuries_expectation_pm, 300);
    assert_eq!(rules.starting_needs_pm["noble"], [1000, 700, 300]);
    assert_eq!(
        rules.diet_profiles["noble"],
        BTreeMap::from([
            ("oats".to_owned(), 250),
            ("barley".to_owned(), 100),
            ("dairy".to_owned(), 250),
            ("beef".to_owned(), 120),
            ("pork".to_owned(), 100),
            ("mutton".to_owned(), 50),
            ("peas_beans".to_owned(), 40),
            ("vegetables".to_owned(), 40),
            ("fish".to_owned(), 50),
        ])
    );
    assert_eq!(
        rules.comfort_profiles["noble"]["niceties"],
        BTreeMap::from([
            ("ale".to_owned(), 600),
            ("mead".to_owned(), 200),
            ("honey".to_owned(), 100),
            ("apples".to_owned(), 100),
        ])
    );
    assert_eq!(
        rules.comfort_profiles["noble"]["luxuries"],
        BTreeMap::from([
            ("mead".to_owned(), 600),
            ("honey".to_owned(), 200),
            ("apples".to_owned(), 200),
        ])
    );
    assert_eq!(rules.ticking_rates_million["herbs"]["noble"], 2000);
    assert_eq!(rules.ticking_rates_million["beeswax"]["noble"], 2000);
}

#[test]
fn the_demesne_loads_at_exactly_the_approved_values_and_the_old_buildings_are_unchanged() {
    let rules = rules();
    let demesne = building(&rules, "demesne");
    assert_eq!(ordinary_places_per_level(demesne), 10);
    assert_eq!(demesne.land_per_level, 2);
    assert_eq!(demesne.methods[0].jobs[0].id, "steward");
    assert_eq!(demesne.penalty_pm, 500);
    assert_eq!(demesne.overhead_pm, 300);
    assert_eq!(demesne.recruit_range_m, Some(30_000));
    assert_eq!(rules.footprint_m2["demesne"], 4000);
    assert_eq!(rules.housing_per_level["demesne"], 37);
    assert_eq!(rules.housing_class["demesne"], "basic");
    assert_eq!(demesne.methods.len(), 1);
    let method = &demesne.methods[0];
    assert_eq!(method.id, "demesne_farming");
    assert_eq!(method.resource, "usable_land");
    assert_eq!(method.suitability, "crops");
    assert_eq!(
        method.annual_output,
        BTreeMap::from(
            [
                ("oats", 300),
                ("barley", 80),
                ("peas_beans", 6),
                ("vegetables", 20),
                ("beef", 1300),
                ("pork", 970),
                ("mutton", 165),
                ("dairy", 1000),
                ("wool", 10),
                ("hides", 8),
                ("ale", 300),
                ("honey", 30),
                ("mead", 60),
                ("apples", 20),
            ]
            .map(|(id, units)| (id.to_owned(), units))
        )
    );
    assert_eq!(
        method.score_share_pm,
        BTreeMap::from(
            [
                ("oats", 430),
                ("barley", 110),
                ("peas_beans", 20),
                ("vegetables", 30),
                ("beef", 40),
                ("pork", 20),
                ("mutton", 5),
                ("dairy", 20),
                ("wool", 5),
                ("hides", 5),
                ("ale", 150),
                ("honey", 40),
                ("mead", 100),
                ("apples", 25),
            ]
            .map(|(id, share)| (id.to_owned(), share))
        )
    );

    assert_eq!(
        building(&rules, "holdings").methods[0]
            .jobs
            .iter()
            .map(|job| u64::from(job.places))
            .sum::<u64>(),
        11
    );
    assert_eq!(building(&rules, "holdings").penalty_pm, 500);
    assert_eq!(building(&rules, "holdings").overhead_pm, 300);
}

#[test]
fn a_job_must_name_a_known_population_type() {
    let mut rules = rules();
    let index = rules
        .buildings
        .iter()
        .position(|item| item.id == "demesne")
        .unwrap();
    rules.buildings[index].methods[0].jobs[0].pop_types = vec!["ghost".to_owned()];
    let error = rules.validate("fixture/economy.toml").unwrap_err();
    assert_eq!(error.entry, format!("buildings[{index}].jobs.steward"));
    assert_eq!(
        error.problem,
        "job needs positive places and wage weight and known eligible types"
    );
}

// ---------------------------------------------------------------- start fixtures

fn pasture(cells: u32, fertility_each: u64) -> LandCounts {
    LandCounts {
        land_cells: BTreeMap::from([("pasture".to_owned(), cells)]),
        fertility_pm: BTreeMap::from([("pasture".to_owned(), u64::from(cells) * fertility_each)]),
        fertile_cells: cells,
        fertile_cells_by_terrain: BTreeMap::from([("pasture".to_owned(), cells)]),
        sea: 0,
    }
}

fn house(culture: &str, people: u64, levels: u32) -> NobleStart {
    NobleStart {
        culture: culture.to_owned(),
        religion: "christianity".to_owned(),
        people,
        demesne_levels: levels,
        labourers: u64::from(levels) * 10,
    }
}

/// One ringfort `fort` with 100 peasants and the given labourers, holdings and farm levels.
fn start_data(
    noble: Option<NobleStart>,
    labourers: u64,
    holdings_levels: u32,
    farm_levels: u32,
) -> ScenarioEconomy {
    ScenarioEconomy {
        horses_always: Vec::new(),
        defaults: BTreeMap::from([(
            "ringfort".to_owned(),
            BTreeMap::from([(
                "1".to_owned(),
                StartDefaults {
                    peasants: 100,
                    monks: 0,
                    labourers,
                    farm_levels,
                    holdings_levels,
                    storehouse_levels: 0,
                    pop_stock: BTreeMap::from([("oats".to_owned(), 3), ("dairy".to_owned(), 1)]),
                    building_stock: BTreeMap::from([("oats".to_owned(), 2)]),
                },
            )]),
        )]),
        places: BTreeMap::from([(
            "fort".to_owned(),
            StartOverride {
                culture: Some("ulaid".to_owned()),
                religion: Some("christianity".to_owned()),
                monk_religion: Some("christianity".to_owned()),
                ..StartOverride::default()
            },
        )]),
        nobles: noble
            .into_iter()
            .map(|noble| ("fort".to_owned(), noble))
            .collect(),
    }
}

fn start(
    rules: &Rules,
    data: &ScenarioEconomy,
    land: LandCounts,
) -> Result<(EconomyState, Vec<String>), DataError> {
    starting_state(
        rules,
        data,
        &[StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land,
        }],
        9,
    )
}

fn kind_index(place: &PlaceState, kind: &str) -> usize {
    place.pops.iter().position(|pop| pop.kind == kind).unwrap()
}

fn building_index(place: &PlaceState, kind: &str) -> usize {
    place
        .buildings
        .iter()
        .position(|item| item.kind == kind)
        .unwrap()
}

// ---------------------------------------------------------------- the start

#[test]
fn a_noble_house_starts_with_its_pop_a_demesne_and_labourers_on_the_demesne_levels() {
    let rules = rules();
    let data = start_data(Some(house("dal_riatan", 40, 2)), 0, 0, 0);
    let (state, report) = start(&rules, &data, pasture(40, 600)).unwrap();
    assert!(report.is_empty(), "{report:?}");
    let place = &state.places[0];

    let nobles = &place.pops[kind_index(place, "noble")];
    assert_eq!(
        (nobles.culture.as_str(), nobles.religion.as_str()),
        ("dal_riatan", "christianity")
    );
    assert_eq!(
        (nobles.workers, nobles.dependents),
        (24, 16),
        "40 heads at 400‰ dependents"
    );
    assert_eq!(
        nobles.stock[good(&rules, "oats")],
        120,
        "the start stock of 40 people"
    );

    // Two levels add the smallest household with 20 workers: 32 heads, 20 workers and 12 dependents.
    let labourers = &place.pops[kind_index(place, "labourer")];
    assert_eq!(
        (labourers.culture.as_str(), labourers.religion.as_str()),
        ("ulaid", "christianity")
    );
    assert_eq!((labourers.workers, labourers.dependents), (20, 12));
    assert_eq!(
        labourers.stock[good(&rules, "oats")],
        96,
        "the start stock of 32 people"
    );
    assert_eq!(labourers.stock[good(&rules, "dairy")], 32);

    let demesne_index = building_index(place, "demesne");
    let demesne = &place.buildings[demesne_index];
    assert_eq!(demesne.levels, 2);
    assert_eq!(
        demesne.stock[good(&rules, "oats")],
        2,
        "the building's start stock"
    );
    // Nobles fill the manager places; labourers fill the ordinary jobs.
    assert_eq!(
        demesne.employment[kind_index(place, "noble")],
        Employment {
            jobs: BTreeMap::from([("steward".into(), 2)]),
            workers: 2,
        }
    );
    assert_eq!(
        demesne.employment[kind_index(place, "labourer")],
        Employment {
            jobs: BTreeMap::from([
                ("field_hands".into(), 8),
                ("milkers".into(), 4),
                ("herder".into(), 2),
                ("butcher".into(), 2),
                ("beekeeper".into(), 2),
                ("brewer".into(), 2),
            ]),
            workers: 20,
        }
    );
    assert_eq!(
        manager_factor(
            building(&rules, "demesne"),
            demesne,
            &vec![0; place.pops.len()],
            1
        ),
        Ok(1000)
    );
}

#[test]
fn the_demesne_takes_land_after_holdings_and_before_the_farm() {
    let rules = rules();
    // Land for three levels (2000 of fertility each): holdings want 1, the demesne 2 and the farm 4.
    // The lord's land comes before the free farmers', so the farm is left with none.
    let data = start_data(Some(house("ulaid", 40, 2)), 10, 1, 4);
    let (state, report) = start(&rules, &data, pasture(12, 500)).unwrap();
    let place = &state.places[0];
    let levels = |kind: &str| {
        place
            .buildings
            .iter()
            .find(|item| item.kind == kind)
            .map(|item| item.levels)
    };
    assert_eq!(levels("holdings"), Some(1));
    assert_eq!(levels("demesne"), Some(2));
    assert_eq!(levels("farm"), Some(0));
    assert_eq!(
        report,
        ["fort farm: cut levels 4 -> 0 (no method has enough land or sea)"]
    );
    assert_eq!(
        place
            .buildings
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>(),
        ["holdings", "demesne", "farm"]
    );
}

#[test]
fn a_demesne_cut_to_the_land_adds_fewer_labourers_and_reports_the_cut() {
    let rules = rules();
    let data = start_data(Some(house("ulaid", 60, 3)), 0, 0, 0);
    // 4500 of fertility is two levels' worth at 2000 each, so the third authored level is cut.
    let (state, report) = start(&rules, &data, pasture(9, 500)).unwrap();
    assert_eq!(
        report,
        ["fort demesne: cut levels 3 -> 2 (no method has enough land or sea)"]
    );
    let place = &state.places[0];
    assert_eq!(place.buildings[building_index(place, "demesne")].levels, 2);
    let labourers = &place.pops[kind_index(place, "labourer")];
    assert_eq!(
        (labourers.workers, labourers.dependents),
        (20, 12),
        "two levels' household, not three levels'"
    );
    assert_eq!(
        place.buildings[building_index(place, "demesne")].employment[kind_index(place, "labourer")]
            .workers,
        20
    );
    assert_eq!(
        place.pops[kind_index(place, "noble")].workers
            + place.pops[kind_index(place, "noble")].dependents,
        60
    );

    // A level cut away entirely adds no labourer pop at all.
    let (state, report) = start(&rules, &data, pasture(1, 500)).unwrap();
    assert_eq!(
        report,
        ["fort demesne: cut levels 3 -> 0 (no method has enough land or sea)"]
    );
    assert!(
        state.places[0]
            .pops
            .iter()
            .all(|pop| pop.kind != "labourer")
    );
}

#[test]
fn guaranteed_seat_requires_a_noble_and_room_for_horses() {
    let rules = rules();
    let mut no_noble = start_data(None, 0, 0, 0);
    no_noble.horses_always.push("fort".into());
    assert!(
        start(&rules, &no_noble, pasture(40, 600))
            .unwrap_err()
            .entry
            .contains("fort")
    );
    let mut cut = start_data(Some(house("ulaid", 60, 3)), 0, 0, 0);
    cut.horses_always.push("fort".into());
    assert!(
        start(&rules, &cut, pasture(1, 500))
            .unwrap_err()
            .entry
            .contains("fort")
    );
}

#[test]
fn the_household_is_the_smallest_whose_workers_reach_the_target() {
    let rules = rules();
    for (target, heads) in [(0, 0), (10, 16), (20, 32), (30, 49), (40, 66)] {
        assert_eq!(
            labourer_household_heads(&rules, target),
            Ok(heads),
            "{target} workers"
        );
        if heads > 0 {
            assert!(heads - heads * 400 / 1000 >= target);
            assert!(
                heads - 1 - (heads - 1) * 400 / 1000 < target,
                "one head fewer falls short"
            );
        }
    }
}

#[test]
fn the_start_household_joins_the_places_existing_labourer_pop_as_one_cohort() {
    let rules = rules();
    // 20 labourers already live here: 12 workers and 8 dependents. They are not the demesne's.
    let data = start_data(Some(house("ulaid", 40, 2)), 20, 1, 0);
    let (state, report) = start(&rules, &data, pasture(40, 600)).unwrap();
    assert!(report.is_empty(), "{report:?}");
    let place = &state.places[0];
    let cohorts = place
        .pops
        .iter()
        .filter(|pop| pop.kind == "labourer")
        .count();
    assert_eq!(
        cohorts, 1,
        "one labourer pop for the place's culture and religion"
    );
    let labourers = &place.pops[kind_index(place, "labourer")];
    assert_eq!((labourers.workers, labourers.dependents), (12 + 20, 8 + 12));
    assert_eq!(
        labourers.stock[good(&rules, "oats")],
        60 + 96,
        "20 + 32 people's start stock"
    );

    // It works in both buildings: the demesne's twenty are reserved before the holdings' five.
    let holdings = &place.buildings[building_index(place, "holdings")];
    let demesne = &place.buildings[building_index(place, "demesne")];
    let index = kind_index(place, "labourer");
    assert_eq!(
        demesne.employment[index].workers, 20,
        "exactly the demesne's positions"
    );
    assert_eq!(
        holdings.employment[index].workers, 5,
        "the holdings' own labourer positions"
    );
}

#[test]
fn a_noble_house_needs_a_known_culture_a_known_religion_and_positions_that_match_its_levels() {
    let rules = rules();
    let land = pasture(40, 600);
    let mut data = start_data(Some(house("atlantis", 20, 1)), 0, 0, 0);
    let error = start(&rules, &data, land.clone()).unwrap_err();
    assert_eq!(
        (error.entry.as_str(), error.problem.as_str()),
        ("nobles.fort.culture", "unknown key `atlantis`")
    );
    let error = data
        .validate_cultures(&rules.peoples, "fixture/economy.toml")
        .unwrap_err();
    assert_eq!(
        (error.entry.as_str(), error.problem.as_str()),
        ("nobles.fort.culture", "unknown culture `atlantis`")
    );

    let mut noble = house("ulaid", 20, 1);
    noble.religion = "sun_worship".to_owned();
    data = start_data(Some(noble), 0, 0, 0);
    let error = start(&rules, &data, land.clone()).unwrap_err();
    assert_eq!(
        (error.entry.as_str(), error.problem.as_str()),
        ("nobles.fort.religion", "unknown key `sun_worship`")
    );

    let mut noble = house("ulaid", 20, 1);
    noble.labourers = 9;
    let error = start(&rules, &start_data(Some(noble), 0, 0, 0), land).unwrap_err();
    assert_eq!(error.entry, "nobles.fort.labourers");
}

// ---------------------------------------------------------------- the restored culture checks (CK-356 N1)

#[test]
fn the_starting_state_refuses_an_unknown_place_monk_or_peasant_culture() {
    let rules = rules();
    let land = pasture(40, 600);
    for (field, entry, problem) in [
        ("culture", "places.fort.culture", "unknown key `atlantis`"),
        (
            "monk_culture",
            "places.fort.monk_culture",
            "unknown key `atlantis`",
        ),
        (
            "peasant_share",
            "places.fort.peasant_cultures_pm.atlantis",
            "unknown culture",
        ),
    ] {
        let mut data = start_data(None, 0, 0, 0);
        let place = data.places.get_mut("fort").unwrap();
        match field {
            "culture" => place.culture = Some("atlantis".to_owned()),
            "monk_culture" => place.monk_culture = Some("atlantis".to_owned()),
            _ => {
                place.peasant_cultures_pm = Some(BTreeMap::from([("atlantis".to_owned(), 1000)]));
            }
        }
        let error = start(&rules, &data, land.clone()).unwrap_err();
        assert_eq!(
            (error.entry.as_str(), error.problem.as_str()),
            (entry, problem),
            "{field}"
        );
    }
}

// ---------------------------------------------------------------- jobs, management and a monthly pass

fn september_first() -> Date {
    Date {
        year: 768,
        month: 9,
        day: 1,
        hour: 12,
    }
}

/// The produced units of oats of one monthly production pass.
fn oats_produced(rules: &Rules, state: &mut EconomyState) -> (u64, ProductionLedger) {
    let ledger = run_monthly_production(rules, state, september_first()).unwrap();
    (ledger.produced[good(rules, "oats")], ledger)
}

#[test]
fn empty_steward_positions_reduce_output_while_field_hands_still_earn_pay() {
    let rules = rules();
    let data = start_data(Some(house("ulaid", 40, 2)), 0, 0, 0);
    let (state, _) = start(&rules, &data, pasture(40, 600)).unwrap();
    let place = &state.places[0];
    let demesne_index = building_index(place, "demesne");
    let nobles = kind_index(place, "noble");
    let labourers = kind_index(place, "labourer");
    let oats = good(&rules, "oats");

    let mut full = state.clone();
    let (full_oats, _) = oats_produced(&rules, &mut full);
    assert!(full_oats > 0);

    // The same pass with steward positions empty: the manager factor applies the 500‰ penalty.
    let mut without = state.clone();
    let place = &mut without.places[0];
    place.buildings[demesne_index].employment[nobles]
        .jobs
        .remove("steward");
    place.buildings[demesne_index].employment[nobles].workers = 0;
    place.pops[nobles].employment[demesne_index] =
        place.buildings[demesne_index].employment[nobles].clone();
    let efficiency = manager_factor(
        building(&rules, "demesne"),
        &place.buildings[demesne_index],
        &vec![0; place.pops.len()],
        1,
    );
    assert_eq!(efficiency, Ok(500));
    let before = place.pops[labourers].stock[oats];
    let (scaled_oats, _) = oats_produced(&rules, &mut without);
    assert!(
        (scaled_oats * 2..=scaled_oats * 2 + 1).contains(&full_oats),
        "500‰ penalty without stewards: {scaled_oats} against {full_oats}"
    );
    // The labourers are still paid: they work and earn their share of the output.
    let paid = without.places[0].pops[labourers].stock[oats] - before;
    assert!(paid > 0, "the labourers' pay arrives without stewards");
    let mut full_paid_state = state.clone();
    let before = full_paid_state.places[0].pops[labourers].stock[oats];
    oats_produced(&rules, &mut full_paid_state);
    assert!(full_paid_state.places[0].pops[labourers].stock[oats] - before > paid);
}

#[test]
fn monthly_hiring_prefers_available_nobles_for_steward_places() {
    let rules = rules();
    let data = start_data(Some(house("ulaid", 80, 4)), 0, 0, 0);
    let (mut state, _) = start(&rules, &data, pasture(60, 600)).unwrap();
    let place = &mut state.places[0];
    let demesne_index = building_index(place, "demesne");
    let nobles = kind_index(place, "noble");
    let labourers = kind_index(place, "labourer");
    assert_eq!(
        place.buildings[demesne_index].employment[nobles]
            .jobs
            .get("steward")
            .copied()
            .unwrap_or(0),
        4
    );

    // Release the stewards; the monthly hiring cap fills from available nobles first.
    place.buildings[demesne_index].employment[nobles]
        .jobs
        .remove("steward");
    place.buildings[demesne_index].employment[nobles].workers = 0;
    place.pops[nobles].employment[demesne_index] =
        place.buildings[demesne_index].employment[nobles].clone();
    fill_monthly_hiring(&rules, place).unwrap();
    assert_eq!(
        place.buildings[demesne_index].employment[nobles]
            .jobs
            .get("steward")
            .copied()
            .unwrap_or(0),
        1
    );
    assert_eq!(
        place.buildings[demesne_index].employment[labourers]
            .jobs
            .get("steward")
            .copied()
            .unwrap_or(0),
        0
    );
    assert_eq!(
        place.pops[labourers].employment[demesne_index]
            .jobs
            .get("steward")
            .copied()
            .unwrap_or(0),
        0
    );
    for _ in 0..40 {
        fill_monthly_hiring(&rules, place).unwrap();
    }
    assert_eq!(
        place.buildings[demesne_index].employment[nobles]
            .jobs
            .get("steward")
            .copied()
            .unwrap_or(0),
        4,
        "one per ten labourers"
    );
    assert_eq!(
        place.buildings[demesne_index].employment[labourers].workers,
        40
    );
}

// ---------------------------------------------------------------- the demesne's store

#[test]
fn an_employed_demesne_keeps_its_overhead_in_the_holdings_sized_store() {
    let rules = rules();
    let data = start_data(Some(house("ulaid", 20, 1)), 0, 0, 0);
    let (mut state, _) = start(&rules, &data, pasture(40, 600)).unwrap();
    let place = &mut state.places[0];
    let demesne_index = building_index(place, "demesne");
    let oats = good(&rules, "oats");

    // The same capacities as a holdings with the same workers, in volume and per good.
    let mut as_holdings = place.buildings[demesne_index].clone();
    as_holdings.kind = "holdings".to_owned();
    assert_eq!(
        store_capacity_ml(&place.buildings[demesne_index]),
        Ok(11 * 1_170_000)
    );
    assert_eq!(
        store_capacity_ml(&place.buildings[demesne_index]),
        store_capacity_ml(&as_holdings)
    );
    for index in 0..rules.goods.len() {
        assert_eq!(
            building_good_cap(&rules, &place.buildings[demesne_index], index),
            building_good_cap(&rules, &as_holdings, index)
        );
    }

    place.buildings[demesne_index].stock.fill(0);
    let before = place.buildings[demesne_index].stock[oats];
    run_monthly_production(&rules, &mut state, september_first()).unwrap();
    let after = state.places[0].buildings[demesne_index].stock[oats];
    assert!(
        after > before,
        "the 300‰ overhead stays in the demesne's own store"
    );
}

#[test]
fn with_the_demesne_store_full_the_excess_goes_on_by_the_overflow_route() {
    let rules = rules();
    let data = start_data(Some(house("ulaid", 20, 1)), 0, 0, 0);
    let (mut state, _) = start(&rules, &data, pasture(40, 600)).unwrap();
    let oats = good(&rules, "oats");
    let place = &mut state.places[0];
    let demesne_index = building_index(place, "demesne");
    let labourers = kind_index(place, "labourer");
    let cap = building_good_cap(&rules, &place.buildings[demesne_index], oats).unwrap();
    place.buildings[demesne_index].stock[oats] = cap;
    place.pops[labourers].stock[oats] = 0;
    let total_before = total_stored_goods(&rules, place).unwrap()[oats];

    let ledger = run_monthly_production(&rules, &mut state, september_first()).unwrap();
    let place = &state.places[0];
    assert_eq!(
        place.buildings[demesne_index].stock[oats], cap,
        "the store stays full"
    );
    // The overhead and the pay both reached the labourers (alms, then wages); nothing is lost on the way.
    assert!(place.pops[labourers].stock[oats] > 0);
    assert_eq!(
        total_stored_goods(&rules, place).unwrap()[oats],
        total_before + ledger.produced[oats] - ledger.lost[oats]
    );
}

// ---------------------------------------------------------------- the 768 start data

#[test]
fn the_shipped_768_start_data_holds_34_noble_houses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let rules = Rules::load(root.join("game/economy.toml")).unwrap();
    let data = ScenarioEconomy::load(root.join("scenarios/ireland-768/economy.toml")).unwrap();
    data.validate_cultures(&rules.peoples, "economy.toml")
        .unwrap();
    // CK-370 added Carraig Aille (20 nobles, one level, ten labourers) to the 33 houses of the P1 start.
    assert_eq!(data.nobles.len(), 34);
    assert_eq!(
        data.nobles.values().map(|noble| noble.people).sum::<u64>(),
        1200
    );
    assert_eq!(
        data.nobles
            .values()
            .map(|noble| noble.labourers)
            .sum::<u64>(),
        600
    );
    assert_eq!(
        data.nobles
            .values()
            .map(|noble| u64::from(noble.demesne_levels))
            .sum::<u64>(),
        60
    );
    for (id, noble) in &data.nobles {
        assert!(
            data.places.contains_key(id),
            "{id} is a place with an economy entry"
        );
        assert_eq!(noble.religion, "christianity", "{id}");
        assert!(
            [20, 40, 80, 120].contains(&noble.people),
            "{id}: {}",
            noble.people
        );
        assert_eq!(
            u64::from(noble.demesne_levels),
            noble.people / 20,
            "{id}: one level per 20 nobles"
        );
        assert_eq!(
            noble.labourers,
            10 * u64::from(noble.demesne_levels),
            "{id}"
        );
    }
    // Dún Sobhairce stands in for Dál Riata: a dal_riatan house in an ulaid place.
    let dunseverick = &data.nobles["dunseverick"];
    assert_eq!(dunseverick.culture, "dal_riatan");
    assert_eq!(data.places["dunseverick"].culture.as_deref(), Some("ulaid"));
    let carraig = &data.nobles["carraig_aille"];
    assert_eq!(
        (
            carraig.culture.as_str(),
            carraig.people,
            carraig.demesne_levels,
            carraig.labourers
        ),
        ("mumu", 20, 1, 10)
    );
    let armagh = &data.nobles["armagh"];
    assert_eq!(
        (
            armagh.culture.as_str(),
            armagh.people,
            armagh.demesne_levels
        ),
        ("airgialla", 20, 1)
    );
    let ailech = &data.nobles["ailech"];
    assert_eq!(
        (
            ailech.culture.as_str(),
            ailech.people,
            ailech.demesne_levels
        ),
        ("northern_ui_neill", 80, 4)
    );
}

// ---------------------------------------------------------------- housing: the pop × building allocation

#[test]
fn the_people_table_conserves_both_totals_and_follows_the_tie_order() {
    // Two pops of one person and two buildings of one place: every cell ties at remainder 1, so the
    // first pop, in the order given, takes the first building.
    assert_eq!(
        allocate_people(&[1, 1], &[1, 1], &[0, 1]).unwrap(),
        [[1, 0], [0, 1]]
    );
    assert_eq!(
        allocate_people(&[1, 1], &[1, 1], &[1, 0]).unwrap(),
        [[0, 1], [1, 0]]
    );

    // Proportions first: 5 people over places of 3 and 7 give the floors, then the largest remainders.
    let table = allocate_people(&[2, 3], &[3, 2], &[0, 1]).unwrap();
    assert_eq!(
        table
            .iter()
            .map(|row| row.iter().sum::<u64>())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(
        (0..2)
            .map(|column| table.iter().map(|row| row[column]).sum::<u64>())
            .collect::<Vec<_>>(),
        [3, 2]
    );
    // The shares are 1.2, 0.8, 1.8 and 1.2: floors 1, 0, 1 and 1, then the two largest remainders (the
    // .8 cells) take the two units left.
    assert_eq!(table, [[1, 1], [2, 1]]);

    // An empty table and unbalanced totals.
    assert_eq!(allocate_people(&[0, 0], &[0], &[0, 1]).unwrap(), [[0], [0]]);
    assert!(allocate_people(&[2], &[1], &[0]).is_err());
}

#[test]
fn the_table_balances_for_every_small_set_of_totals_in_either_order() {
    for (a, b, c) in
        (0..=4).flat_map(|a| (0..=4).flat_map(move |b| (0..=4).map(move |c| (a, b, c))))
    {
        let rows = [a, b, c];
        let total = a + b + c;
        for first in 0..=total {
            let columns = [first, total - first];
            for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
                let table = allocate_people(&rows, &columns, &order).unwrap();
                for (row, wanted) in table.iter().zip(rows) {
                    assert_eq!(row.iter().sum::<u64>(), wanted, "{rows:?} {columns:?}");
                }
                for (column, wanted) in columns.into_iter().enumerate() {
                    assert_eq!(
                        table.iter().map(|row| row[column]).sum::<u64>(),
                        wanted,
                        "{rows:?} {columns:?}"
                    );
                }
            }
        }
    }
}

/// A rules set where the housing of the demesne and the farm is a single place a level.
fn rules_with_small_housing() -> Rules {
    let mut rules = rent_rules();
    rules.housing_per_level.insert("demesne".to_owned(), 1);
    rules.housing_per_level.insert("farm".to_owned(), 1);
    rules
}

struct Tenant {
    kind: &'static str,
    culture: &'static str,
    workers: u64,
    dependents: u64,
    stock: Vec<(&'static str, u64)>,
    flagged: bool,
    /// `(building index, owners, workers)` for every building the pop is employed in.
    jobs: Vec<(usize, u64, u64)>,
}

impl Tenant {
    fn new(kind: &'static str, culture: &'static str, workers: u64) -> Self {
        Self {
            kind,
            culture,
            workers,
            dependents: 0,
            stock: Vec::new(),
            flagged: false,
            jobs: Vec::new(),
        }
    }

    fn with_stock(mut self, stock: &[(&'static str, u64)]) -> Self {
        self.stock = stock.to_vec();
        self
    }

    fn flagged(mut self) -> Self {
        self.flagged = true;
        self
    }

    fn dependents(mut self, dependents: u64) -> Self {
        self.dependents = dependents;
        self
    }

    fn job(mut self, building: usize, owners: u64, workers: u64) -> Self {
        self.jobs.push((building, owners, workers));
        self
    }
}

fn building_state(rules: &Rules, kind: &str, levels: u32, pops: usize) -> BuildingState {
    let goods = rules.goods.len();
    let methods = building(rules, kind).methods.len();
    BuildingState {
        heir: None,
        abandoned: false,
        kind: kind.to_owned(),
        levels,
        method_levels: vec![0; methods],
        level_records: Vec::new(),
        herds: Vec::new(),
        employment: vec![Employment::default(); pops],
        stock: vec![0; goods],
        storehouse_accounts: None,
        production_carries: vec![vec![0; goods]; methods],
        method_suitability: vec![vec![0; goods]; methods],
        herd_birth_carries: Vec::new(),
        herd_death_carries: Vec::new(),
        herd_product_carries: Vec::new(),
        herd_grazing_sums: Vec::new(),
        wage_carries: vec![0; goods],
        pay_carries: vec![0; goods],
        last_made: Vec::new(),
        fee_carries: Vec::new(),
        wage_history: vec![0; 12],
        paid_history: vec![0; WAGE_HISTORY_MONTHS],
        paid_years: vec![0; WAGE_HISTORY_MONTHS],
    }
}

/// A place of the given buildings (kind, levels) and tenants, priced as given in milli-units a good.
fn place_of(
    rules: &Rules,
    buildings: &[(&str, u32)],
    tenants: Vec<Tenant>,
    prices: &[(&str, u64)],
) -> PlaceState {
    let goods = rules.goods.len();
    let mut place = PlaceState {
        id: "fixture".to_owned(),
        pops: Vec::new(),
        buildings: buildings
            .iter()
            .map(|(kind, levels)| building_state(rules, kind, *levels, tenants.len()))
            .collect(),
        prices_milli: rules
            .goods
            .iter()
            .map(|item| item.base_price * 1000)
            .collect(),
        prepared_m2: 0,
        development_pm: 0,
        price_drift_carries: vec![0; goods],
        last_market_orders: None,
        counters: CounterBuckets::for_place(goods),
        trade: PlaceTrade::default(),
    };
    for (id, price) in prices {
        place.prices_milli[good(rules, id)] = *price;
    }
    for (pop_index, tenant) in tenants.into_iter().enumerate() {
        let mut stock = vec![0; goods];
        for (id, units) in &tenant.stock {
            stock[good(rules, id)] = *units;
        }
        let mut employment = vec![Employment::default(); buildings.len()];
        for (building_index, owners, workers) in &tenant.jobs {
            let kind = place.buildings[*building_index].kind.as_str();
            let (job, count) = match (kind, tenant.kind) {
                ("demesne", "noble") => ("steward", (*workers).max(*owners)),
                ("farm", "peasant") => ("peasant_labour", (*workers).max(*owners)),
                ("holdings", "monk") => ("monks", *workers),
                _ => ("field_hands", *workers),
            };
            employment[*building_index] = Employment {
                jobs: if count == 0 {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([(job.to_owned(), count)])
                },
                workers: count,
            };
            place.buildings[*building_index].employment[pop_index] =
                employment[*building_index].clone();
        }
        place.pops.push(PopState {
            kind: tenant.kind.to_owned(),
            culture: tenant.culture.to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: tenant.workers,
            dependents: tenant.dependents,
            employment,
            stock,
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0; goods],
                ticking_carries: vec![0; goods],
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: tenant.flagged,
            housed_last: tenant.flagged,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        });
    }
    place
}

#[test]
fn a_short_pop_is_housed_last_in_the_spare_step_behind_those_who_paid_in_full() {
    let rules = rules_with_small_housing();
    let make = |flagged: bool| {
        let unflagged = Tenant::new("labourer", "a", 1);
        let short = Tenant::new("labourer", "b", 2);
        place_of(
            &rules,
            &[("demesne", 1)],
            vec![unflagged, if flagged { short.flagged() } else { short }],
            &[],
        )
    };
    // One spare place, an unflagged pop with one unhoused person and a flagged pop with two.
    let fill = housing_fill(&rules, &make(true), 10).unwrap();
    assert_eq!(
        (fill.pops[0].basic, fill.pops[0].makeshift),
        (1, 0),
        "the unflagged person gets the place"
    );
    assert_eq!((fill.pops[1].basic, fill.pops[1].makeshift), (0, 2));
    // Without the flag the plain award gives it to the pop with the larger remainder, the second.
    let plain = housing_fill(&rules, &make(false), 10).unwrap();
    assert_eq!(
        (plain.pops[0].basic, plain.pops[1].basic),
        (0, 1),
        "today's award alone"
    );
    assert_eq!(plain.housed[1], [1]);
}

#[test]
fn with_spare_places_for_everyone_the_flag_changes_nothing() {
    let rules = rules();
    let tenants = |flagged| {
        vec![
            Tenant::new("labourer", "a", 3),
            if flagged {
                Tenant::new("labourer", "b", 4).flagged()
            } else {
                Tenant::new("labourer", "b", 4)
            },
        ]
    };
    let one = housing_fill(
        &rules,
        &place_of(&rules, &[("demesne", 1)], tenants(true), &[]),
        0,
    )
    .unwrap();
    let two = housing_fill(
        &rules,
        &place_of(&rules, &[("demesne", 1)], tenants(false), &[]),
        0,
    )
    .unwrap();
    assert_eq!(one, two);
    assert_eq!(one.housed, [[3], [4]]);
}

#[test]
fn two_pops_two_buildings_each_with_one_spare_place_only_the_demesne_tenant_pays() {
    let rules = rules_with_small_housing();
    let prices = [("oats", 2000), ("barley", 1000)];
    let place =
        |tenants: Vec<Tenant>| place_of(&rules, &[("demesne", 1), ("farm", 1)], tenants, &prices);
    let labourer = || Tenant::new("labourer", "a", 1).with_stock(&[("oats", 10)]);
    let peasant = || Tenant::new("peasant", "b", 1).with_stock(&[("barley", 10)]);

    let fill = housing_fill(&rules, &place(vec![labourer(), peasant()]), 0).unwrap();
    assert_eq!(
        fill.housed,
        [[1, 0], [0, 1]],
        "the first pop takes the first building on the tie"
    );
    assert_eq!(fill.building_occupants, [1, 1]);
    assert_eq!(
        fill.pops.iter().map(|pop| pop.basic).collect::<Vec<_>>(),
        [1, 1]
    );

    // Flagged pops come after the others in the tie order: the peasant, unflagged, takes the demesne.
    let flagged_first = place(vec![labourer().flagged(), peasant()]);
    let fill = housing_fill(&rules, &flagged_first, 0).unwrap();
    assert_eq!(fill.housed, [[0, 1], [1, 0]]);

    // Rent: only the person in the demesne pays; the farm's tenant pays nothing.
    let mut place = place(vec![labourer(), peasant()]);
    let fill = housing_fill(&rules, &place, 0).unwrap();
    let mut ledger =
        PlaceProductionLedger::new("fixture", rules.goods.len(), rules.herd_species.len());
    ledger.stock_after = total_stored_goods(&rules, &place).unwrap();
    collect_monthly_rent(&rules, &mut place, &fill, 30, &mut ledger).unwrap();
    assert_eq!(
        paid_and_short(&place.pops[0]),
        (2000, 1000),
        "3 due, one unit of oats at 2"
    );
    assert_eq!(place.pops[1].counters.current, MonthlyCounters::default());
    assert_eq!(place.pops[1].stock[good(&rules, "barley")], 10);
}

// ---------------------------------------------------------------- rent

/// Shipped rules with simple one-good diets so the rent has worked numbers: a person's month of
/// necessities is `days` units of oats (labourers, nobles) or barley (peasants).
fn rent_rules() -> Rules {
    let mut rules = rules();
    for id in ["oats", "barley"] {
        let index = good(&rules, id);
        rules.goods[index].hunger_centi = 100;
    }
    for (profile, id) in [
        ("labourer", "oats"),
        ("noble", "oats"),
        ("peasant", "barley"),
    ] {
        rules
            .diet_profiles
            .insert(profile.to_owned(), BTreeMap::from([(id.to_owned(), 1000)]));
    }
    rules
}

/// Run the rent pass on a place once, as the monthly pass does after the housing fill.
fn pay_rent(
    rules: &Rules,
    place: &mut PlaceState,
    shelter: u64,
    days: u64,
) -> PlaceProductionLedger {
    let fill = housing_fill(rules, place, shelter).unwrap();
    let mut ledger =
        PlaceProductionLedger::new(&place.id, rules.goods.len(), rules.herd_species.len());
    ledger.stock_after = total_stored_goods(rules, place).unwrap();
    collect_monthly_rent(rules, place, &fill, days, &mut ledger).unwrap();
    ledger
}

/// What the monthly pass does to the flags on the 1st, before the housing fill.
fn first_of_month(place: &mut PlaceState) {
    for pop in &mut place.pops {
        pop.housed_last = pop.short_on_rent;
    }
}

fn paid_and_short(pop: &PopState) -> (u64, u64) {
    (
        pop.counters.current.rent_paid_milli,
        pop.counters.current.rent_short_milli,
    )
}

#[test]
fn the_rent_basket_is_one_persons_necessities_for_the_month_at_local_prices() {
    let rules = rent_rules();
    let place = place_of(&rules, &[], vec![], &[("oats", 1300), ("barley", 1000)]);
    // 30 days at 100 hundredths of a person-day a unit: 30 units of oats at 1.3 each is 39.
    assert_eq!(
        rent_basket_milli(&rules, &place, "labourer", 30),
        Ok(39_000)
    );
    assert_eq!(rent_basket_milli(&rules, &place, "peasant", 30), Ok(30_000));
    assert_eq!(
        rent_basket_milli(&rules, &place, "labourer", 28),
        Ok(36_400)
    );
    // Each good rounds up to whole units before it is priced: 7 days of the real oats (18 a unit) is 1.
    let mut slow = rules.clone();
    let oats = good(&slow, "oats");
    slow.goods[oats].hunger_centi = 1800;
    assert_eq!(rent_basket_milli(&slow, &place, "labourer", 7), Ok(1300));
}

#[test]
fn a_labourer_pop_housed_workers_first_in_a_demesne_pays_a_twentieth_of_the_basket_per_head() {
    let rules = rent_rules();
    let prices = [("oats", 1300)];
    // The pop of 10 workers and 6 dependents, all in the demesne's housing: 16 people, V = 39.
    let labourers = Tenant::new("labourer", "a", 10)
        .dependents(6)
        .with_stock(&[("oats", 100)])
        .job(0, 0, 10);
    let nobles = Tenant::new("noble", "b", 12)
        .dependents(8)
        .with_stock(&[("oats", 100)])
        .job(0, 1, 0);
    let mut place = place_of(&rules, &[("demesne", 1)], vec![labourers, nobles], &prices);
    let ledger = pay_rent(&rules, &mut place, 0, 30);
    let oats = good(&rules, "oats");

    // ⌊16 × 39 / 20⌋ = 31 value units due; the proportional share of 100 oats is 23 units (29.9), and
    // what is owed after it, 1.1, is below one unit: residue, forgiven. The pop is not short.
    assert_eq!(paid_and_short(&place.pops[0]), (29_900, 1100));
    assert_eq!(place.pops[0].stock[oats], 77);
    assert!(!place.pops[0].short_on_rent);
    assert_eq!(
        place.buildings[0].stock[oats], 23,
        "the rent arrives in the building's stockpile"
    );
    assert_eq!(ledger.lost[oats], 0);
    assert_eq!(ledger.stock_after[oats], 200, "nothing left the place");

    // The nobles who own the demesne pay nothing, though they live in it.
    assert_eq!(place.pops[1].counters.current, MonthlyCounters::default());
    assert_eq!(place.pops[1].stock[oats], 100);
}

#[test]
fn two_labourers_with_a_basket_of_39_owe_three() {
    let rules = rent_rules();
    // V = 39 and n = 2 give ⌊2 × 39 / 20⌋ = 3 value units due.
    let labourers = Tenant::new("labourer", "a", 2)
        .with_stock(&[("oats", 10)])
        .job(0, 0, 2);
    let mut place = place_of(
        &rules,
        &[("demesne", 1)],
        vec![labourers],
        &[("oats", 1300)],
    );
    pay_rent(&rules, &mut place, 0, 30);
    // 10 oats are worth 13: more than 3, so the proportional share is 2 units (2.6) and 0.4 is forgiven.
    let (paid, short) = paid_and_short(&place.pops[0]);
    assert_eq!((paid, short), (2600, 400));
    assert_eq!(paid + short, 3000, "the due is three whole value units");
    assert!(!place.pops[0].short_on_rent);
}

#[test]
fn peasants_in_a_farm_and_nobles_in_their_own_demesne_pay_nothing() {
    let mut rules = rent_rules();
    rules.housing_per_level.insert("farm".to_owned(), 16);
    let peasants = Tenant::new("peasant", "a", 10)
        .dependents(6)
        .with_stock(&[("barley", 50)])
        .job(0, 1, 10);
    let nobles = Tenant::new("noble", "b", 12)
        .dependents(8)
        .with_stock(&[("oats", 50)])
        .job(1, 1, 0);
    let mut place = place_of(
        &rules,
        &[("farm", 1), ("demesne", 1)],
        vec![peasants, nobles],
        &[],
    );
    pay_rent(&rules, &mut place, 0, 30);
    for pop in &place.pops {
        assert_eq!(pop.counters.current, MonthlyCounters::default());
        assert!(!pop.short_on_rent);
    }
    assert_eq!(place.pops[0].stock[good(&rules, "barley")], 50);
    assert_eq!(place.pops[1].stock[good(&rules, "oats")], 50);
    assert_eq!(place.buildings[1].stock, vec![0; rules.goods.len()]);
}

#[test]
fn spare_housed_people_pay_where_they_land_and_a_mixed_overflow_charges_only_the_demesne_tenant() {
    let rules = rules_with_small_housing();
    let prices = [("oats", 2000), ("barley", 1000)];
    // A noble owner fills the demesne's only place and a farm owner the farm's. A peasant and a
    // labourer, each one person with no work, have nowhere: one overflows into each building.
    let tenants = vec![
        Tenant::new("noble", "n", 1).job(0, 1, 0),
        Tenant::new("peasant", "f", 1).job(1, 1, 0),
        Tenant::new("peasant", "p", 1).with_stock(&[("barley", 10)]),
        Tenant::new("labourer", "l", 1).with_stock(&[("oats", 10)]),
    ];
    let mut place = place_of(&rules, &[("demesne", 1), ("farm", 1)], tenants, &prices);
    let fill = housing_fill(&rules, &place, 0).unwrap();
    // The stated allocation: the peasant, first in the pop order, is in the demesne; the labourer in the farm.
    assert_eq!(fill.housed, [[1, 0], [0, 1], [1, 0], [0, 1]]);
    pay_rent(&rules, &mut place, 0, 30);

    // Only the peasant pays, by its own diet: V = 30 barley at 1.0, n = 1, ⌊30 / 20⌋ = 1 value unit,
    // a unit of barley. The labourer's own basket (60) would have charged 3.
    assert_eq!(paid_and_short(&place.pops[2]), (1000, 0));
    assert_eq!(place.pops[2].stock[good(&rules, "barley")], 9);
    // The steward is paid, so the demesne has storage for the rent.
    assert_eq!(place.buildings[0].stock[good(&rules, "barley")], 1);
    assert_eq!(place.pops[3].stock[good(&rules, "barley")], 0);
    assert_eq!(place.pops[3].counters.current, MonthlyCounters::default());
    assert_eq!(place.pops[3].stock[good(&rules, "oats")], 10);
}

#[test]
fn a_full_demesne_store_with_two_tenants_debits_both_before_the_receipt_goes_on_by_the_overflow_route()
 {
    let rules = rent_rules();
    let prices = [("oats", 1000), ("ale", 5000)];
    // Four people each, V = 28 on a 28-day month: ⌊4 × 28 / 20⌋ = 5 value units owed by each.
    let first = Tenant::new("labourer", "a", 4).job(0, 0, 4);
    let second = Tenant::new("labourer", "b", 4)
        .with_stock(&[("ale", 1)])
        .job(0, 0, 4);
    let mut place = place_of(&rules, &[("demesne", 1)], vec![first, second], &prices);
    let ale = good(&rules, "ale");
    let cap = building_good_cap(&rules, &place.buildings[0], ale).unwrap();
    place.buildings[0].stock[ale] = cap;
    let before = total_stored_goods(&rules, &place).unwrap();

    let ledger = pay_rent(&rules, &mut place, 0, 28);
    // The first has nothing: short, flagged, pays nothing. The second's one unit is worth exactly 5: paid whole.
    assert_eq!(paid_and_short(&place.pops[0]), (0, 5000));
    assert!(place.pops[0].short_on_rent);
    assert_eq!(paid_and_short(&place.pops[1]), (5000, 0));
    assert!(
        !place.pops[1].short_on_rent,
        "a store worth exactly the due is not short"
    );
    // The store was full, so the unit went on by the overflow route (alms: labourers first, in pop
    // order) after both debits: the first, short tenant received it, and it did not fund its own rent.
    assert_eq!(place.buildings[0].stock[ale], cap);
    assert_eq!(place.pops[1].stock[ale], 0);
    assert_eq!(place.pops[0].stock[ale], 1);
    // Conservation per good: the debits equal the building's gain plus what the overhead path forwarded.
    assert_eq!(total_stored_goods(&rules, &place).unwrap(), before);
    assert_eq!(ledger.lost, vec![0; rules.goods.len()]);
}

#[test]
fn a_rent_no_store_can_take_is_lost_and_booked_as_the_overheads_loss_is() {
    let mut rules = rent_rules();
    // Ale cannot be kept anywhere here: no building store, no pop store.
    let ale = good(&rules, "ale");
    rules.goods[ale].pop_limit_milli = 0;
    let tenant = Tenant::new("labourer", "a", 4)
        .with_stock(&[("ale", 1)])
        .job(0, 0, 4);
    let mut place = place_of(
        &rules,
        &[("demesne", 1)],
        vec![tenant],
        &[("oats", 1000), ("ale", 5000)],
    );
    let ledger = pay_rent(&rules, &mut place, 0, 28);
    assert_eq!(paid_and_short(&place.pops[0]), (5000, 0));
    assert_eq!(ledger.lost[ale], 1);
    assert_eq!(place.counters.current.losses, 1);
    assert_eq!(place.counters.current.lost[ale], 1);
    assert_eq!(
        ledger.stock_after[ale], 0,
        "the closing stock follows the loss"
    );
}

#[test]
fn rent_is_paid_in_whole_units_and_the_remainder_is_forgiven() {
    // A due of 7 against one unit each of two goods worth 5: the proportional shares are 0 and 0,
    // then one unit of the first (ties in rules order) is taken; 2 are left, below any unit: residue.
    assert_eq!(
        rent_payment_units(&[5000, 5000], &[1, 1], 10_000, 7000).unwrap(),
        [1, 0]
    );
    // A store worth no more than the due is paid whole.
    assert_eq!(
        rent_payment_units(&[5000, 5000], &[1, 1], 10_000, 10_000).unwrap(),
        [1, 1]
    );
    assert_eq!(
        rent_payment_units(&[5000, 5000], &[1, 1], 10_000, 12_000).unwrap(),
        [1, 1]
    );
    // Unequal prices: shares ⌊20·4/36⌋ = 2, ⌊20·5/36⌋ = 2 and ⌊20·2/36⌋ = 1 take 17; of the 3 still owed
    // only the second and third goods have stock left at a price within it, and the cheapest, the
    // second, gives one unit; 1 is left, below any unit.
    assert_eq!(
        rent_payment_units(&[3000, 2000, 7000], &[4, 5, 2], 36_000, 20_000).unwrap(),
        [2, 3, 1]
    );
    // The cheapest unit that fits goes first, taken as many times as fits in one step: a share of 4 units
    // of the first good, then the 2 still owed in one step.
    assert_eq!(
        rent_payment_units(&[1000, 4000], &[10, 1], 14_000, 6000).unwrap(),
        [6, 0]
    );
    // A good with no stock is never taken.
    assert_eq!(
        rent_payment_units(&[1000, 4000], &[0, 5], 20_000, 8000).unwrap(),
        [0, 2]
    );
}

#[test]
fn a_due_of_7_against_two_goods_worth_5_pays_one_unit_forgives_2_and_does_not_flag_the_pop() {
    let rules = rent_rules();
    // Value 7 due: n = 1 and V = 140 give ⌊140 / 20⌋ = 7 on a 28-day month at 5 an oat unit.
    let tenant = Tenant::new("labourer", "a", 1)
        .with_stock(&[("oats", 1), ("ale", 1)])
        .job(0, 0, 1);
    let mut place = place_of(
        &rules,
        &[("demesne", 1)],
        vec![tenant],
        &[("oats", 5000), ("ale", 5000)],
    );
    pay_rent(&rules, &mut place, 0, 28);
    assert_eq!(
        paid_and_short(&place.pops[0]),
        (5000, 2000),
        "paid 5, short 2 (the residue)"
    );
    assert!(!place.pops[0].short_on_rent, "residue alone does not flag");
    let oats = good(&rules, "oats");
    let ale = good(&rules, "ale");
    assert_eq!(
        (place.pops[0].stock[oats], place.pops[0].stock[ale]),
        (0, 1),
        "one unit, in rules order"
    );
}

#[test]
fn a_pop_that_cannot_pay_pays_what_it_has_is_flagged_and_housed_last_until_a_clean_month() {
    let rules = {
        let mut rules = rent_rules();
        rules.housing_per_level.insert("demesne".to_owned(), 1);
        rules
    };
    let prices = [("oats", 2000)];
    // One spare place; both labourers have one person. V = 60, so one person owes 3 value units.
    let poor = Tenant::new("labourer", "a", 1).with_stock(&[("oats", 1)]);
    let rich = Tenant::new("labourer", "b", 1).with_stock(&[("oats", 10)]);
    let mut place = place_of(&rules, &[("demesne", 1)], vec![poor, rich], &prices);
    let fill = housing_fill(&rules, &place, 5).unwrap();
    assert_eq!(
        fill.housed,
        [[1], [0]],
        "a tie in the award goes to the first pop"
    );

    // Month 1: the first pays everything it has (a unit, 2) against 3 due, and is flagged.
    pay_rent(&rules, &mut place, 5, 30);
    assert_eq!(paid_and_short(&place.pops[0]), (2000, 1000));
    assert!(place.pops[0].short_on_rent);
    assert!(!place.pops[1].short_on_rent);
    // The demesne has no paid worker here, so its store holds nothing: the debited unit went on by the
    // alms route (labourers first), after the debit, and came back to the only labourer in line.
    assert_eq!(
        total_stored_goods(&rules, &place).unwrap()[good(&rules, "oats")],
        11
    );

    // The same month's live housing still applies last month's priority: the result waits for the 1st.
    assert_eq!(housing_fill(&rules, &place, 5).unwrap().housed, [[1], [0]]);

    // Month 2: on the 1st the result takes effect. The flagged pop is housed last; the other gets
    // the place and pays. The first, in makeshift shelter, owes nothing, so its flag clears.
    first_of_month(&mut place);
    let fill = housing_fill(&rules, &place, 5).unwrap();
    assert_eq!(fill.housed, [[0], [1]]);
    pay_rent(&rules, &mut place, 5, 30);
    assert!(
        !place.pops[0].short_on_rent,
        "a month without a shortfall clears the flag"
    );
    assert!(place.pops[1].counters.current.rent_paid_milli > 0);
    assert!(!place.pops[1].short_on_rent);

    // Month 3: no flag is left, and the first pop is first in the tie again.
    assert_eq!(
        housing_fill(&rules, &place, 5).unwrap().housed,
        [[0], [1]],
        "still month 2's priority"
    );
    first_of_month(&mut place);
    let fill = housing_fill(&rules, &place, 5).unwrap();
    assert_eq!(fill.housed, [[1], [0]]);
}

#[test]
fn the_rent_counters_roll_with_the_existing_counters_on_the_first_and_are_saved() {
    let rules = rent_rules();
    let tenant = Tenant::new("labourer", "a", 2)
        .with_stock(&[("oats", 1)])
        .job(0, 0, 2);
    let mut place = place_of(&rules, &[("demesne", 1)], vec![tenant], &[("oats", 1300)]);
    pay_rent(&rules, &mut place, 0, 30);
    // 3 due, one 1.3 unit held: paid 1.3, short 1.7 (the whole store is paid; the rest is unpaid).
    assert_eq!(paid_and_short(&place.pops[0]), (1300, 1700));
    assert!(place.pops[0].short_on_rent);

    // They are saved with the flag, byte for byte.
    let state = EconomyState {
        places: vec![place.clone()],
        trade: TradeCounters::default(),
    };
    let loaded = EconomyState::from_bytes(&state.to_bytes().unwrap()).unwrap();
    assert_eq!(loaded, state);
    assert_eq!(paid_and_short(&loaded.places[0].pops[0]), (1300, 1700));
    assert!(loaded.places[0].pops[0].short_on_rent);

    // The next first of the month rolls them into `last` and starts `current` from zero, where the
    // births and deaths roll.
    let mut state = loaded;
    run_monthly_production(
        &rules,
        &mut state,
        Date {
            year: 768,
            month: 10,
            day: 1,
            hour: 12,
        },
    )
    .unwrap();
    let counters = &state.places[0].pops[0].counters;
    assert_eq!(
        (
            counters.last.rent_paid_milli,
            counters.last.rent_short_milli
        ),
        (1300, 1700)
    );
    assert_eq!(
        (
            counters.current.rent_paid_milli,
            counters.current.rent_short_milli
        ),
        (0, 0)
    );
    assert!(counters.closed);
    assert!(
        state.places[0].pops[0].short_on_rent,
        "the flag persists until the next rent pass"
    );
}

// ---------------------------------------------------------------- determinism

fn next_day(date: Date) -> Date {
    let length = days_in_month(date.year, date.month).unwrap();
    if date.day < length {
        Date {
            day: date.day + 1,
            ..date
        }
    } else if date.month < 12 {
        Date {
            month: date.month + 1,
            day: 1,
            ..date
        }
    } else {
        Date {
            year: date.year + 1,
            month: 1,
            day: 1,
            ..date
        }
    }
}

fn run_days(rules: &Rules, state: &mut EconomyState, from: Date, days: u32) -> Date {
    let shelter = BTreeMap::from([("fort".to_owned(), 30)]);
    let mut date = from;
    for _ in 0..days {
        run_daily_economy_with_context(
            rules,
            state,
            date,
            &BTreeMap::new(),
            &shelter,
            &BTreeMap::new(),
        )
        .unwrap();
        date = next_day(date);
    }
    date
}

fn rent_total(state: &EconomyState) -> u64 {
    state
        .places
        .iter()
        .flat_map(|place| &place.pops)
        .map(|pop| pop.counters.current.rent_paid_milli + pop.counters.last.rent_paid_milli)
        .sum()
}

#[test]
fn a_two_month_run_with_rent_is_the_same_twice_and_a_save_load_in_the_middle_changes_nothing() {
    let rules = rules();
    let data = start_data(Some(house("ulaid", 40, 2)), 20, 1, 3);
    let (start_state, _) = start(&rules, &data, pasture(60, 600)).unwrap();
    let from = Date {
        year: 768,
        month: 8,
        day: 20,
        hour: 12,
    };

    let mut first = start_state.clone();
    run_days(&rules, &mut first, from, 62);
    let mut second = start_state.clone();
    run_days(&rules, &mut second, from, 62);
    assert!(
        rent_total(&first) > 0,
        "labourers in the demesne's housing paid rent"
    );
    assert_eq!(
        first.to_bytes().unwrap(),
        second.to_bytes().unwrap(),
        "the same state twice"
    );

    // Save at the middle (after the September 1st pass), load, and carry on: the same end state.
    let mut resumed = start_state;
    let middle = run_days(&rules, &mut resumed, from, 25);
    assert_eq!((middle.month, middle.day), (9, 14));
    let mut resumed = EconomyState::from_bytes(&resumed.to_bytes().unwrap()).unwrap();
    run_days(&rules, &mut resumed, middle, 37);
    assert_eq!(
        first.to_bytes().unwrap(),
        resumed.to_bytes().unwrap(),
        "save and load continue identically"
    );
}

// ---------------------------------------------------------------- the live view and the month boundary

#[test]
fn the_live_housing_read_keeps_the_months_priority_until_the_next_first_and_survives_a_save() {
    let mut rules = rent_rules();
    // The demesne has one paid position, held by a third household, and room for two: a workers-first
    // place for the worker and one spare place, so no one else is hired into it.
    rules.housing_per_level.insert("demesne".to_owned(), 2);
    let index = rules
        .buildings
        .iter()
        .position(|item| item.id == "demesne")
        .unwrap();
    rules.buildings[index].methods[0]
        .jobs
        .retain(|job| job.id == "field_hands");
    rules.buildings[index].methods[0].jobs[0].places = 1;
    rules.buildings[index].methods[0]
        .made_by
        .retain(|_, job| job == "field_hands");
    // Two one-person households with no work contend for the spare place and have five makeshift
    // slots to fall back on. The first has no store, so its rent is short; the second can pay.
    let first = Tenant::new("labourer", "a", 1);
    let second = Tenant::new("labourer", "b", 1).with_stock(&[("oats", 20)]);
    let worker = Tenant::new("labourer", "c", 1)
        .with_stock(&[("oats", 20)])
        .job(0, 0, 1);
    let place = place_of(&rules, &[("demesne", 1)], vec![first, second, worker], &[]);
    let mut state = EconomyState {
        places: vec![place],
        trade: TradeCounters::default(),
    };
    let shelter = BTreeMap::from([("fixture".to_owned(), 5)]);
    let tick = |state: &mut EconomyState, month: u8| {
        let date = Date {
            year: 768,
            month,
            day: 1,
            hour: 12,
        };
        run_daily_economy_with_context(
            &rules,
            state,
            date,
            &BTreeMap::new(),
            &shelter,
            &BTreeMap::new(),
        )
        .unwrap();
    };
    let live = |state: &EconomyState| housing_fill(&rules, &state.places[0], 5).unwrap().housed;
    assert_eq!(
        live(&state),
        [[1], [0], [1]],
        "a tie in the award goes to the first pop"
    );

    // October 1st: the first household gets the spare place, cannot pay and falls short.
    tick(&mut state, 10);
    assert!(
        state.places[0].pops[0].short_on_rent,
        "its shortfall is this month's rent result"
    );
    assert!(
        !state.places[0].pops[0].housed_last,
        "but it takes effect only at the next 1st"
    );
    assert_eq!(
        live(&state),
        [[1], [0], [1]],
        "the same-day read is the housing the tick used"
    );

    // A save in the middle of the month changes nothing, on either side of the boundary.
    let mut reloaded = EconomyState::from_bytes(&state.to_bytes().unwrap()).unwrap();
    assert_eq!(live(&reloaded), [[1], [0], [1]]);

    // November 1st: the result is promoted before the fill, so the short pop is housed last.
    tick(&mut state, 11);
    tick(&mut reloaded, 11);
    assert!(state.places[0].pops[0].housed_last);
    assert_eq!(live(&state), [[0], [1], [1]]);
    assert_eq!(
        state.to_bytes().unwrap(),
        reloaded.to_bytes().unwrap(),
        "save and load continue identically"
    );
}

// ---------------------------------------------------------------- exact rent arithmetic

#[test]
fn a_product_wider_than_128_bits_is_divided_exactly() {
    let top = u128::MAX;
    for (a, b, c, expected) in [
        (
            1_u128 << 100,
            1_u128 << 100,
            1_u128 << 100,
            Some(1_u128 << 100),
        ),
        (top, top, top, Some(top)),
        (
            (1 << 127) + 5,
            3,
            7,
            Some(72_917_650_054_486_813_599_294_558_735_378_902_457),
        ),
        (
            (1 << 127) + 12_345,
            (1 << 126) + 999,
            (1 << 127) + 1,
            Some(85_070_591_730_234_615_865_843_651_857_942_060_035),
        ),
        (
            3_458_764_513_820_540_928_000,
            1 << 60,
            3_458_764_513_820_540_928_000 * 2,
            Some(1 << 59),
        ),
        // The quotient does not fit, or the divisor is zero.
        (top, top - 1, top - 2, None),
        (
            3 * 10_u128.pow(38),
            7 * 10_u128.pow(37),
            5 * 10_u128.pow(30),
            None,
        ),
        (1, 1, 0, None),
        // A product that fits takes the plain path.
        (10, 7, 3, Some(23)),
    ] {
        assert_eq!(mul_div_floor(a, b, c), expected, "{a} * {b} / {c}");
    }
}

#[test]
fn a_proportional_share_of_a_huge_store_is_exact_and_does_not_panic() {
    // 2^60 units each of two goods at 1 and 2: the store is worth 3 × 2^60 × 1000 and half is due.
    // The product of the due and the units does not fit 128 bits, the shares are exactly 2^59 each.
    let units = 1_u64 << 60;
    let store = u128::from(units) * 1000 + u128::from(units) * 2000;
    assert_eq!(
        rent_payment_units(&[1000, 2000], &[units, units], store, store / 2).unwrap(),
        [units / 2, units / 2]
    );
}

#[test]
fn a_rent_counter_past_u64_is_a_named_error_never_a_silent_cap() {
    let rules = rent_rules();
    // 2^56 paying heads with a 39-unit monthly basket and an empty store owe 140512308373959475000
    // thousandths: more than a counter can hold.
    let tenant = Tenant::new("labourer", "a", 1);
    let mut place = place_of(&rules, &[("demesne", 1)], vec![tenant], &[("oats", 1300)]);
    let fill = HousingFill {
        housed: vec![vec![1 << 56]],
        ..HousingFill::default()
    };
    let mut ledger =
        PlaceProductionLedger::new("fixture", rules.goods.len(), rules.herd_species.len());
    ledger.stock_after = total_stored_goods(&rules, &place).unwrap();
    let error = collect_monthly_rent(&rules, &mut place, &fill, 30, &mut ledger).unwrap_err();
    assert_eq!(
        (error.entry.as_str(), error.problem.as_str()),
        ("counters.rent", "rent counter exceeds u64")
    );
}
