//! The 768 start (CK-370): a new game of the shipped scenario holds its real figures, their families and the people of its slots; it is
//! deterministic, and its state saves and loads with its archive.

use std::{fs, path::PathBuf};

use ck_characters::{Archive, Character, Departure, Office, PopLocator, Populations};
use ck_economy::EconomyState;
use ck_log::Level;
use ck_sim::{Canonical, Save};

use crate::{
    config::GameConfig,
    game::{
        GameCommand, GameState, apply,
        character_storage::Storage,
        economy::starting_economy,
        setup::{Prepared, Request, prepare},
    },
    testing::Dir,
};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// A new game of the shipped 768 scenario on `seed`: the scratch directory (the game's storage lives in it), the game and the log.
pub(super) fn shipped(seed: u64) -> (Dir, Prepared, Vec<(Level, String)>) {
    let dir = Dir::new();
    let game = GameConfig {
        scenario: assets().join("scenarios/ireland-768"),
        saves: dir.0.join("saves"),
    };
    let mut records = Vec::new();
    let prepared = prepare(
        &Request {
            game: &game,
            vms: &[],
            load: None,
            seed: Some(seed),
        },
        || Ok(seed),
        &mut |level, message| records.push((level, message)),
    )
    .expect("a new game of the 768 scenario");
    (dir, prepared, records)
}

fn heads(economy: &EconomyState) -> u64 {
    economy
        .places
        .iter()
        .flat_map(|place| &place.pops)
        .map(|pop| pop.workers + pop.dependents)
        .sum()
}

/// A character of the state or the archive called `name` for whom `keep` holds.
fn find(
    state: &GameState,
    archive: &Archive,
    name: &str,
    keep: impl Fn(&Character) -> bool,
) -> Character {
    state
        .characters
        .iter()
        .cloned()
        .chain(archive.ids().filter_map(|id| archive.get(id).unwrap()))
        .find(|c| c.name == name && keep(c))
        .unwrap_or_else(|| panic!("no character called {name}"))
}

fn locator(place: &str, slot: usize) -> PopLocator {
    PopLocator {
        place: place.to_owned(),
        slot: u32::try_from(slot).unwrap(),
    }
}

/// The pops of a place: slot, type, culture, heads.
fn pops_of(state: &GameState, place: &str) -> Vec<(usize, String, String, u64)> {
    state
        .economy
        .places
        .iter()
        .find(|p| p.id == place)
        .unwrap_or_else(|| panic!("no place {place}"))
        .pops
        .iter()
        .enumerate()
        .map(|(slot, pop)| {
            (
                slot,
                pop.kind.clone(),
                pop.culture.clone(),
                pop.workers + pop.dependents,
            )
        })
        .collect()
}

#[test]
fn shipped_herds_start_at_two_thirds_of_their_maximum() {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let (economy, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let rules = ck_economy::Rules::load(assets().join("game/economy.toml")).unwrap();
    let mut checked = 0;
    let mut demesnes = 0;
    for building in economy.places.iter().flat_map(|place| &place.buildings) {
        let definition = rules
            .buildings
            .iter()
            .find(|b| b.id == building.kind)
            .unwrap();
        let levels: u64 = definition
            .methods
            .iter()
            .zip(&building.method_levels)
            .filter(|(method, _)| method.resource != "sea")
            .map(|(_, levels)| u64::from(*levels))
            .sum();
        let horses = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "horses")
            .unwrap();
        if building.kind == "demesne" {
            demesnes += 1;
            assert_eq!(definition.herds["horses"].maximum, 3);
            assert_eq!(building.herds[horses], levels * 3 * 2 / 3);
        } else {
            assert_eq!(building.herds.get(horses).copied().unwrap_or(0), 0);
            assert!(!definition.herds.contains_key("horses"));
        }
        for (species, head) in rules.herd_species.iter().zip(&building.herds) {
            let maximum = definition
                .herds
                .get(&species.animal)
                .map_or(0, |c| c.maximum * levels);
            assert_eq!(
                *head,
                maximum * 2 / 3,
                "{} {}",
                building.kind,
                species.animal
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
    assert!(demesnes > 0);
}

/// A running digest of the economy, with every field a building or a place keeps but the level records (which are new): the
/// stores and the storehouse's per-pop and communal accounts, herds, wages, fees, staffing, the product carries and the
/// production carries in the old unit, and every pop whole.
struct EconomyDigest(u64);

impl std::fmt::Write for EconomyDigest {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        for byte in text.bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        Ok(())
    }
}

/// Production carries come in twelfths of the old unit from CK-406 on.
const CARRY_UNIT: u128 = 12;

fn economy_digest(economy: &EconomyState) -> u64 {
    use std::fmt::Write as _;

    let mut digest = EconomyDigest(0xcbf2_9ce4_8422_2325);
    for place in &economy.places {
        write!(
            digest,
            "{} {:?} {} {} {:?} {:?} {:?}",
            place.id,
            place.prices_milli,
            place.prepared_m2,
            place.development_pm,
            place.price_drift_carries,
            place.counters,
            place.pops
        )
        .unwrap();
        for building in &place.buildings {
            let scaled: Vec<u128> = building
                .production_carries
                .iter()
                .flatten()
                .map(|carry| {
                    let carry = *carry;
                    assert_eq!(carry % CARRY_UNIT, 0);
                    carry / CARRY_UNIT
                })
                .collect();
            write!(
                digest,
                " {} {} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                building.kind,
                building.levels,
                building.method_levels,
                building.herds,
                building.employment,
                building.stock,
                building.storehouse_accounts,
                building.method_suitability,
                building.herd_birth_carries,
                building.herd_death_carries,
                building.herd_product_carries,
                building.herd_grazing_sums,
                building.wage_carries,
                building.pay_carries,
                (
                    &building.fee_carries,
                    &building.wage_history,
                    &building.paid_history,
                    &building.paid_years
                ),
                scaled
            )
            .unwrap();
        }
    }
    digest.0
}

#[test]
fn a_year_of_the_shipped_game_from_its_first_noon_is_day_for_day_the_build_before_levels_were_records()
 {
    use std::fmt::Write as _;

    // The prepared game as the game runs it: its own clock from 00:00 on 9 October 768, its scheduler with each place's mean
    // fertility, shelter, nearby places, the character phases and the population floors. The economy after each noon step, from
    // the first noon through the noon of the next anniversary.
    let (_dir, mut prepared, _) = shipped(42);
    let first = prepared.simulation.date().unwrap();
    assert_eq!(
        (first.year, first.month, first.day, first.hour),
        (768, 10, 9, 0)
    );
    let end = ck_sim::GameTime::from_date(ck_sim::Date {
        year: 769,
        month: 10,
        day: 9,
        hour: 13,
    })
    .unwrap();
    let mut lines = String::new();
    let mut noons = 0_u32;
    while prepared.simulation.time() < end {
        let errors = prepared.simulation.step().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let now = prepared.simulation.date().unwrap();
        if now.hour == 13 {
            writeln!(
                lines,
                "{}-{:02}-{:02} {:016x}",
                now.year,
                now.month,
                now.day,
                economy_digest(&prepared.simulation.state().economy)
            )
            .unwrap();
            noons += 1;
        }
    }
    assert_eq!(
        noons, 366,
        "every noon from 9 October 768 through 9 October 769"
    );
    assert_eq!(lines, include_str!("shipped_game_year_accepted.txt"));
}

#[test]
fn shipped_start_keeps_a_mature_record_for_every_level_and_they_add_up() {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let (economy, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let rules = ck_economy::Rules::load(assets().join("game/economy.toml")).unwrap();
    let mut records = 0;
    for building in economy.places.iter().flat_map(|place| &place.buildings) {
        let definition = rules
            .buildings
            .iter()
            .find(|b| b.id == building.kind)
            .unwrap();
        if definition.methods.is_empty() {
            assert!(building.level_records.is_empty(), "{}", building.kind);
            continue;
        }
        // A level cut at the start has no record: the records are the levels that stand.
        assert_eq!(
            building.level_records.len(),
            building.levels as usize,
            "{}",
            building.kind
        );
        let mut counts = vec![0; definition.methods.len()];
        for (number, record) in building.level_records.iter().enumerate() {
            assert_eq!(record.number as usize, number);
            assert_eq!(record.paydays, ck_economy::MATURE_PAYDAYS);
            counts[record.method as usize] += 1;
            records += 1;
        }
        assert_eq!(counts, building.method_levels, "{}", building.kind);
    }
    assert!(records > 0);
    ck_economy::validate_level_records(&rules, &std::collections::BTreeMap::new(), &economy)
        .unwrap();
}

#[test]
fn shipped_start_assigns_fuel_methods_and_all_farm_levels() {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let (economy, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let mut fuel_levels = 0;
    for place in &economy.places {
        for building in &place.buildings {
            if building.kind == "farm" {
                fuel_levels += building.method_levels[2..].iter().sum::<u32>();
                assert_eq!(building.method_levels.iter().sum::<u32>(), building.levels);
            }
        }
    }
    assert!(fuel_levels > 0);
}

#[test]
fn the_shipped_figures_join_the_scenarios_places_and_pops_before_any_generation() {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let (economy, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let data = ck_characters::figures::FigureData::parse(
        &fs::read_to_string(directory.join("ireland-768.toml")).unwrap(),
        &fs::read_to_string(directory.join("ireland-768-load.toml")).unwrap(),
    )
    .unwrap();
    // Every figure's place and pop type, every dead parent's, and every slot's place and the pop type of each member exist (no slot member
    // without its pop), on the economy as it starts.
    data.check_pops(&Populations::all_pops(&economy)).unwrap();
    // The two additions of the designer's data: Carraig Aille's noble pop, and Clogher's monks.
    let kinds = |place: &str| -> Vec<String> {
        economy
            .places
            .iter()
            .find(|p| p.id == place)
            .unwrap()
            .pops
            .iter()
            .map(|pop| pop.kind.clone())
            .collect()
    };
    assert!(kinds("carraig_aille").contains(&"noble".to_owned()));
    assert!(kinds("clogher").contains(&"monk".to_owned()));
    // The scenario hash covers both files (they are found by the scenario directory's name): the same files in a directory of that name give
    // the same hash, and changing a byte of either changes it.
    let hash = crate::game::hashes::scenario_hash(&directory).unwrap();
    let copy = Dir::new();
    let named = copy.0.join("ireland-768");
    fs::create_dir(&named).unwrap();
    for entry in fs::read_dir(&directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), named.join(entry.file_name())).unwrap();
        }
    }
    assert_eq!(
        crate::game::hashes::scenario_hash(&named).unwrap(),
        hash,
        "the same files, the same hash"
    );
    for file in ["ireland-768.toml", "ireland-768-load.toml", "houses.toml"] {
        let path = named.join(file);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{text}\n# changed\n")).unwrap();
        assert_ne!(
            crate::game::hashes::scenario_hash(&named).unwrap(),
            hash,
            "{file} joins the scenario hash"
        );
        fs::write(&path, text).unwrap();
    }
    assert_eq!(crate::game::hashes::scenario_hash(&named).unwrap(), hash);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one new game of the shipped scenario, asked every question of the spec's tests in turn: each is a few lines"
)]
fn a_new_game_of_the_768_scenario_starts_with_its_real_figures_and_families() {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let (before, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let (_dir, prepared, log) = shipped(7);
    let state = prepared.simulation.state();
    let storage = prepared.storage.path.clone();
    let archive = Archive::open(storage.join("ARCH")).unwrap();

    // The start log reports the totals and each pop the safety net raised.
    let lines: Vec<&str> = log.iter().map(|(_, line)| line.as_str()).collect();
    assert!(
        lines.iter().any(|line| line.contains(
            "characters start: 282 real figures (6 dead before the start), 10 dead parents"
        )),
        "{lines:?}"
    );
    assert!(
        (2500..=3500).contains(&state.characters.iter().count()),
        "about 3,000 live characters: {}",
        state.characters.iter().count()
    );

    // Niall Frossach: his pop and house, his office, his byname and his dates.
    let niall = state
        .characters
        .iter()
        .find(|c| c.life.byname == "Frossach")
        .unwrap()
        .clone();
    assert_eq!(niall.name, "Niall");
    assert_eq!(niall.life.house, "cenel_neogain");
    assert_eq!(niall.office, Some(Office::Ruler));
    assert_eq!(niall.birth.date().unwrap().year, 718);
    assert_eq!(
        pops_of(state, &niall.pop.place)[niall.pop.slot as usize].1,
        "noble"
    );
    assert_eq!(niall.pop.place, "ailech");
    // His living spouse is Dúnlaith, and Eithne, his archived wife, stays archived beside her.
    let dunlaith = find(state, &archive, "Dúnlaith", |_| true);
    assert!(
        state.characters.get(dunlaith.id).is_some(),
        "Dúnlaith is live"
    );
    assert!(niall.spouses.contains(&dunlaith.id));
    let eithne = find(state, &archive, "Eithne", |c| {
        c.spouses.contains(&niall.id) && c.death.is_some()
    });
    assert!(
        state.characters.get(eithne.id).is_none(),
        "Eithne is archived, never live"
    );
    assert!(niall.spouses.contains(&eithne.id));
    assert_eq!(eithne.death.unwrap().date().unwrap().year, 768);
    // His son Áed: the named parents, their lineage, and the rivalry with his father at its authored strength.
    let aed = state
        .characters
        .iter()
        .find(|c| c.life.byname == "Oirdnide")
        .unwrap()
        .clone();
    assert_eq!((aed.father, aed.mother), (niall.id, dunlaith.id));
    assert_eq!(aed.genetics.paternal, niall.genetics.paternal);
    assert_eq!(aed.ties.strength(niall.id, "rival"), 3);
    assert_eq!(niall.ties.strength(aed.id, "rival"), 3);

    // A figure who died on 18 May 768 is archived with that date and never live; his rivalry is in his archived record from its append.
    let fer = find(state, &archive, "Fer dá Chrích", |_| true);
    assert_eq!(
        fer.death
            .map(|d| d.date().unwrap())
            .map(|d| (d.year, d.month, d.day)),
        Some((768, 5, 18))
    );
    assert!(state.characters.get(fer.id).is_none());
    let airechtach = find(state, &archive, "Airechtach", |_| true);
    assert!(state.characters.get(airechtach.id).is_some());
    assert_eq!(fer.ties.strength(airechtach.id, "rival"), 2);
    assert_eq!(
        archive.ids().filter(|id| *id == fer.id).count(),
        1,
        "one record"
    );
    // A dead parent with no recorded death is archived as unknown, through a real reopen of the archive.
    let suibne = find(state, &archive, "Suibne", |c| {
        c.life.departure == Departure::DeathUnknown
    });
    assert_eq!(
        Archive::open(storage.join("ARCH"))
            .unwrap()
            .get(suibne.id)
            .unwrap(),
        Some(suibne)
    );

    // The economy additions: Carraig Aille's noble pop and its generated lord; Clogher's monks; Skellig Michael's twelve monks hold its eleven.
    assert!(
        pops_of(state, "carraig_aille")
            .iter()
            .any(|p| p.1 == "noble" && p.2 == "mumu")
    );
    let lord = state
        .characters
        .iter()
        .find(|c| c.life.role == "lord" && c.pop.place == "carraig_aille")
        .unwrap()
        .clone();
    let wife = lord
        .spouses
        .iter()
        .filter_map(|id| state.characters.get(*id))
        .find(|s| s.life.role == "spouse_of_lord")
        .unwrap();
    assert_eq!(wife.life.slot_key, "carraig_aille_household");
    // Carraig Aille has peasants and is not monastic: the generated lord and the wife who fills the spouse slot are both in its noble pop,
    // where the slot puts them (a relative the generator adds is moved only from a monastic place).
    let noble_pop = lord.pop.clone();
    assert_eq!(
        pops_of(state, "carraig_aille")[noble_pop.slot as usize].1,
        "noble"
    );
    assert_eq!(wife.pop, noble_pop, "the wife lives with her husband");
    assert!(
        pops_of(state, "clogher")
            .iter()
            .any(|p| p.1 == "monk" && p.3 >= 10)
    );
    let skellig = pops_of(state, "skellig_michael");
    let skellig_monks = skellig.iter().find(|p| p.1 == "monk").unwrap();
    assert_eq!(
        skellig_monks.3, 12,
        "twelve monks: the economy's start, with no addition"
    );
    assert_eq!(
        state
            .characters
            .pop_count(&locator("skellig_michael", skellig_monks.0)),
        11,
        "an abbot and ten monks"
    );

    // The safety net: each pop holds at least its characters, and the report's additions are exactly what the economy gained (head moves
    // conserve heads), pop by pop for the pops it names.
    let (mut raised_total, mut named) = (0, 0);
    for line in &lines {
        if let Some(rest) = line.strip_prefix("characters start: pop ") {
            let by: u64 = rest
                .split("raised by ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            raised_total += by;
            named += 1;
        }
    }
    assert!(
        named > 0,
        "the safety net raised some pops: its additions are reported"
    );
    assert_eq!(
        heads(&state.economy),
        heads(&before) + raised_total,
        "heads conserved but for the net's additions"
    );
    state
        .characters
        .validate_pops(|pop| {
            state
                .economy
                .places
                .iter()
                .find(|p| p.id == pop.place)?
                .pops
                .get(pop.slot as usize)
                .map(|p| p.workers + p.dependents)
        })
        .unwrap();

    // Relatives. Monaincha has no peasant pop: its five monks fill its five heads, its labourer pop holds only the people the research put
    // there, and the monks' surviving generated relatives live at the nearest Mumu peasant pop, Roscrea's.
    let monaincha = pops_of(state, "monaincha");
    let (monk_slot, ..) = monaincha.iter().find(|p| p.1 == "monk").unwrap();
    assert_eq!(
        state
            .characters
            .pop_count(&locator("monaincha", *monk_slot)),
        5
    );
    let (labourer_slot, ..) = monaincha.iter().find(|p| p.1 == "labourer").unwrap();
    for c in state
        .characters
        .iter()
        .filter(|c| c.pop == locator("monaincha", *labourer_slot))
    {
        assert!(!c.life.slot_key.is_empty(), "{} is no slot member", c.name);
    }
    let peasant_pop_of = |pop: &PopLocator| {
        let (_, kind, culture, _) = &pops_of(state, &pop.place)[pop.slot as usize];
        (kind.clone(), culture.clone())
    };
    let mut living_parents = 0;
    for monk in state
        .characters
        .iter()
        .filter(|c| c.pop == locator("monaincha", *monk_slot))
    {
        for parent in [monk.father, monk.mother]
            .into_iter()
            .filter_map(|id| state.characters.get(id))
        {
            living_parents += 1;
            assert_eq!(parent.pop.place, "roscrea", "the nearest Mumu peasant pop");
            assert_eq!(
                peasant_pop_of(&parent.pop),
                ("peasant".to_owned(), "mumu".to_owned())
            );
        }
    }
    assert!(
        living_parents > 0,
        "the monks have living parents (the seed is fixed), so the rule is exercised"
    );
    // The island places (r15): their labourer pops hold only their slot members, no generated parent lives there and the safety net raises
    // neither; a craftsman's living generated parents live at the nearest Mumu peasant pop.
    let reach = crate::game::setup::marriage_reach(&scenario, u64::MAX);
    for place in ["illaunloughan", "church_island_currane"] {
        assert!(
            !lines
                .iter()
                .any(|line| line.contains(&format!("pop {place}/"))),
            "{place} is raised by the safety net"
        );
        let nearest = reach[place]
            .iter()
            .find(|near| {
                pops_of(state, &near.id)
                    .iter()
                    .any(|p| p.1 == "peasant" && p.2 == "mumu")
            })
            .map(|near| near.id.clone())
            .unwrap();
        let (labourers, ..) = pops_of(state, place)
            .into_iter()
            .find(|p| p.1 == "labourer")
            .unwrap();
        let here = locator(place, labourers);
        let members: Vec<&Character> = state.characters.iter().filter(|c| c.pop == here).collect();
        assert!(!members.is_empty());
        assert!(
            members.iter().all(|c| !c.life.slot_key.is_empty()),
            "{place}: only its slot members"
        );
        let mut parents = 0;
        for member in members {
            for parent in [member.father, member.mother]
                .into_iter()
                .filter_map(|id| state.characters.get(id))
            {
                parents += 1;
                assert_eq!(
                    parent.pop.place, nearest,
                    "{place}: the nearest Mumu peasant pop"
                );
                assert_eq!(peasant_pop_of(&parent.pop).0, "peasant");
            }
        }
        assert!(parents > 0 || place == "church_island_currane", "{place}");
    }
    // Mayo's Northumbrian monks: a surviving generated parent is abroad (archived with that state, no pop or head, never rolled for death).
    let mayo = pops_of(state, "mayo");
    let (mayo_monks, ..) = mayo
        .iter()
        .find(|p| p.1 == "monk" && p.2 == "northumbrian")
        .unwrap();
    let mut abroad = 0;
    for monk in state
        .characters
        .iter()
        .filter(|c| c.pop == locator("mayo", *mayo_monks))
    {
        for parent in [monk.father, monk.mother] {
            assert!(
                state.characters.get(parent).is_none(),
                "no live relative of a Northumbrian monk"
            );
            let record = archive.get(parent).unwrap().unwrap();
            if record.life.departure == Departure::Abroad {
                abroad += 1;
                assert_eq!(record.death, None);
                assert!(record.archivable());
            } else {
                assert!(record.death.is_some());
            }
        }
    }
    assert!(abroad > 0, "some parents of Mayo's monks are abroad");

    // Dead figures take no head and no live pop: nothing archived is live.
    assert!(archive.ids().all(|id| state.characters.get(id).is_none()));
}

#[test]
fn two_new_games_on_one_seed_are_the_same_and_another_seed_differs_and_the_start_saves_and_loads() {
    let digest = |prepared: &Prepared| {
        let mut bytes = prepared.simulation.state().to_bytes().unwrap();
        bytes.extend(fs::read(prepared.storage.path.join("ARCH")).unwrap());
        bytes.extend(fs::read(prepared.storage.path.join("LOGS")).unwrap());
        ck_sim::fnv1a(&bytes)
    };
    let (_a_dir, a, a_log) = shipped(21);
    let (_b_dir, b, b_log) = shipped(21);
    let (_c_dir, c, _) = shipped(22);
    assert_eq!(
        digest(&a),
        digest(&b),
        "the same seed: the same state, archive and dated logs"
    );
    assert_ne!(digest(&a), digest(&c), "another seed differs");
    let report = |log: &[(Level, String)]| -> Vec<String> {
        log.iter()
            .filter(|(_, line)| line.starts_with("characters start"))
            .map(|(_, line)| line.clone())
            .collect()
    };
    assert_eq!(report(&a_log), report(&b_log));

    // Save the start state with its archive and logs, and load it: the state, the archive and the logs come back.
    let seed = 21;
    let save = a.simulation.save(a.identity.meta(seed));
    let mut archive_file = fs::File::open(a.storage.path.join("ARCH")).unwrap();
    let mut log_file = fs::File::open(a.storage.path.join("LOGS")).unwrap();
    let (archive_length, log_length) = (
        archive_file.metadata().unwrap().len(),
        log_file.metadata().unwrap().len(),
    );
    let mut bytes = Vec::new();
    save.write_with_sections(
        &mut bytes,
        &mut [
            (*b"ARCH", archive_length, &mut archive_file),
            (*b"LOGS", log_length, &mut log_file),
        ],
    )
    .unwrap();
    let staging = Storage::fresh_path(&a_dir_parent(&a)).unwrap();
    let (loaded, _) = Save::<GameState>::read_with_sections(
        &mut std::io::Cursor::new(&bytes),
        &a.identity.expected(),
        &staging,
        &[*b"ARCH", *b"LOGS"],
    )
    .unwrap();
    let staged = Storage::at(staging, None, None, None);
    Storage::validate(&staged.path, &loaded.state).unwrap();
    assert_eq!(&loaded.state, a.simulation.state());
    let original = Archive::open(a.storage.path.join("ARCH")).unwrap();
    let restored = Archive::open(staged.path.join("ARCH")).unwrap();
    assert_eq!(
        original.ids().collect::<Vec<_>>(),
        restored.ids().collect::<Vec<_>>()
    );
    for id in original.ids() {
        assert_eq!(original.get(id).unwrap(), restored.get(id).unwrap());
    }
    assert_eq!(
        fs::read(a.storage.path.join("LOGS")).unwrap(),
        fs::read(staged.path.join("LOGS")).unwrap()
    );
    // The restored simulation resumes from the loaded state.
    let resumed = ck_sim::Simulation::<GameState, GameCommand>::from_save(
        loaded,
        ck_sim::Scheduler::new(),
        apply,
    )
    .unwrap();
    assert_eq!(resumed.state(), a.simulation.state());
    // CK-390 rule 9: the canonical state and its digest survive a save and a load in this build.
    assert_eq!(
        resumed.digest().unwrap(),
        a.simulation.digest().unwrap(),
        "a load keeps the digest"
    );
}

/// The scratch directory a prepared game's storage is in.
fn a_dir_parent(prepared: &Prepared) -> PathBuf {
    prepared.storage.path.parent().unwrap().to_path_buf()
}

/// A copy of `from` (a directory tree) at `to`.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// An assets root in `dir` holding the shipped rules and map and the 768 scenario as a directory called `name`.
fn assets_with_scenario(dir: &Dir, name: &str) -> PathBuf {
    let root = dir.0.join("assets");
    copy_tree(&assets().join("game"), &root.join("game"));
    copy_tree(&assets().join("map"), &root.join("map"));
    copy_tree(
        &assets().join("scenarios/ireland-768"),
        &root.join("scenarios").join(name),
    );
    root.join("scenarios").join(name)
}

fn new_game_in(dir: &Dir, scenario: PathBuf, seed: u64) -> Result<Prepared, String> {
    let game = GameConfig {
        scenario,
        saves: dir.0.join("saves"),
    };
    prepare(
        &Request {
            game: &game,
            vms: &[],
            load: None,
            seed: Some(seed),
        },
        || Ok(seed),
        &mut |_, _| {},
    )
}

#[test]
fn half_a_figure_pair_is_refused_and_a_relocated_scenario_still_starts_with_its_characters() {
    // Either file missing is an error, for the hash and for the game: never a start of Ireland without its characters.
    for missing in ["ireland-768-load.toml", "ireland-768.toml"] {
        let dir = Dir::new();
        let scenario = assets_with_scenario(&dir, "ireland-768");
        fs::remove_file(scenario.join(missing)).unwrap();
        let error = crate::game::hashes::scenario_hash(&scenario).unwrap_err();
        assert!(error.contains(&format!("missing {missing}")), "{error}");
        let refused = new_game_in(&dir, scenario, 7)
            .err()
            .expect("a refused start");
        assert!(refused.contains(&format!("missing {missing}")), "{refused}");
    }
    // The same scenario in a directory of another name is found by its own id: the same hash, and the same characters.
    let original =
        crate::game::hashes::scenario_hash(&assets().join("scenarios/ireland-768")).unwrap();
    let dir = Dir::new();
    let moved = assets_with_scenario(&dir, "somewhere-else");
    assert_eq!(
        crate::game::hashes::scenario_hash(&moved).unwrap(),
        original,
        "the hash follows the scenario, not its folder"
    );
    let prepared = new_game_in(&dir, moved, 7).expect("a relocated start");
    let count = prepared.simulation.state().characters.iter().count();
    assert!(
        (2500..=3500).contains(&count),
        "the relocated scenario has its characters: {count}"
    );
    // Both files missing under the Ireland id is an error too: never a start of Ireland without its characters.
    let neither = Dir::new();
    let scenario = assets_with_scenario(&neither, "ireland-768");
    fs::remove_file(scenario.join("ireland-768.toml")).unwrap();
    fs::remove_file(scenario.join("ireland-768-load.toml")).unwrap();
    let refused = new_game_in(&neither, scenario, 7)
        .err()
        .expect("a refused start");
    assert!(
        refused.contains("ireland-768.toml, ireland-768-load.toml"),
        "{refused}"
    );
    // A scenario of another id is outside the start whatever files its folder holds: none, and no error.
    let other = Dir::new();
    let scenario = assets_with_scenario(&other, "ireland-768");
    let toml = fs::read_to_string(scenario.join("scenario.toml")).unwrap();
    fs::write(
        scenario.join("scenario.toml"),
        toml.replace("ireland_768", "some_other_768"),
    )
    .unwrap();
    let with_files = new_game_in(&other, scenario.clone(), 7).expect("another id starts");
    assert_eq!(
        with_files.simulation.state().characters.iter().count(),
        0,
        "the figures of another id are not loaded"
    );
    fs::remove_file(scenario.join("ireland-768.toml")).unwrap();
    fs::remove_file(scenario.join("ireland-768-load.toml")).unwrap();
    let without = new_game_in(&other, scenario, 7).expect("another id without figures starts");
    assert_eq!(without.simulation.state().characters.iter().count(), 0);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the start run by hand, then every pop reconciled against the log, the ledger's moves and the opening"
)]
fn the_safety_net_report_reconciles_each_pop_with_its_head_moves_and_the_rest_of_the_opening_is_unchanged()
 {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let (before, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let rules = ck_economy::Rules::load(assets().join("game/economy.toml")).unwrap();
    let data = ck_characters::figures::FigureData::parse(
        &fs::read_to_string(directory.join("ireland-768.toml")).unwrap(),
        &fs::read_to_string(directory.join("ireland-768-load.toml")).unwrap(),
    )
    .unwrap();
    let start = ck_sim::GameTime::from_date(ck_sim::Date {
        year: 768,
        month: 10,
        day: 9,
        hour: 0,
    })
    .unwrap();
    // The start run by hand, so that the ledger that makes the head moves can be asked what it did.
    let scratch = Dir::new();
    let (names, reach) = (
        crate::game::setup::place_names(&scenario),
        crate::game::setup::marriage_reach(&scenario, u64::MAX),
    );
    let mut economy = before.clone();
    let mut archive = Archive::open(scratch.0.join("ARCH")).unwrap();
    let mut logs = ck_characters::LifeLog::open(scratch.0.join("LOGS")).unwrap();
    let (report, raised, moved, characters) = {
        let ledger = ck_characters::Ledger::with_nearby(&mut economy, &reach)
            .with_place_names(&names)
            .with_start_safety_net();
        let mut world = ck_characters::World {
            rules: &rules.events,
            personality: &rules.personality,
            genetics: &rules.genetics,
            lifecycle: &rules.lifecycle,
            seed: 7,
            populations: &ledger,
            archive: &mut archive,
            logs: &mut logs,
        };
        let mut characters = ck_characters::CharacterStore::default();
        let report = characters.start_768(&mut world, &data, start).unwrap();
        (report, ledger.raised(), ledger.moved(), characters)
    };
    assert_eq!(
        report.raised, raised,
        "the report is the ledger's own count"
    );
    assert!(!raised.is_empty() && !moved.is_empty());
    // The log of a new game on the same seed names exactly these pops with exactly these increments.
    let (_dir, prepared, log) = shipped(7);
    let mut logged = std::collections::BTreeMap::new();
    for (_, line) in &log {
        if let Some(rest) = line.strip_prefix("characters start: pop ") {
            let (pop, rest) = rest.split_once(" raised by ").unwrap();
            let (place, slot) = pop.split_once('/').unwrap();
            let by: u64 = rest.split(' ').next().unwrap().parse().unwrap();
            logged.insert((place.to_owned(), slot.parse::<u32>().unwrap()), by);
        }
    }
    let ours: std::collections::BTreeMap<(String, u32), u64> = raised
        .iter()
        .map(|(pop, n)| ((pop.place.clone(), pop.slot), *n))
        .collect();
    assert_eq!(
        logged, ours,
        "the start log names the same pops with the same increments"
    );
    assert_eq!(
        prepared.simulation.state().economy,
        economy,
        "the game's start is the one run by hand"
    );
    // Reconcile every pop: heads at the end are the heads at the start, plus what the net added, plus the heads that moved in, less those out.
    for (place_after, place_before) in economy.places.iter().zip(&before.places) {
        assert_eq!(place_after.id, place_before.id);
        for (slot, (after, was)) in place_after.pops.iter().zip(&place_before.pops).enumerate() {
            let pop = PopLocator {
                place: place_after.id.clone(),
                slot: u32::try_from(slot).unwrap(),
            };
            let added = ours
                .get(&(pop.place.clone(), pop.slot))
                .copied()
                .unwrap_or(0);
            let moved_in = moved.iter().filter(|(_, to)| *to == pop).count() as u64;
            let moved_out = moved.iter().filter(|(from, _)| *from == pop).count() as u64;
            let (final_heads, initial) = (
                after.workers + after.dependents,
                was.workers + was.dependents,
            );
            assert_eq!(
                final_heads,
                initial + added + moved_in - moved_out,
                "{}/{}",
                pop.place,
                pop.slot
            );
            let placed = characters.pop_count(&pop) as u64;
            assert!(
                final_heads >= placed,
                "{}/{}: {final_heads} heads, {placed} characters",
                pop.place,
                pop.slot
            );
            assert_eq!(
                (&after.kind, &after.culture, &after.religion),
                (&was.kind, &was.culture, &was.religion)
            );
            // A pop nothing touched is exactly as it opened.
            if added == 0 && moved_in == 0 && moved_out == 0 {
                assert_eq!(after, was, "{}/{} was not touched", pop.place, pop.slot);
            }
        }
        // Whatever else a place holds is as it opened. A head that moved releases the employed position it held (the cohort rule of a death), so
        // in the places a head moved through only the buildings' employment may differ; everywhere else the whole place but its pops is the same.
        let touched = moved
            .iter()
            .any(|(from, to)| from.place == place_after.id || to.place == place_after.id);
        let (mut after, mut was) = (place_after.clone(), place_before.clone());
        (after.pops, was.pops) = (Vec::new(), Vec::new());
        if touched {
            for building in after.buildings.iter_mut().chain(was.buildings.iter_mut()) {
                building.employment = Vec::new();
            }
        }
        assert!(
            after == was,
            "{} apart from its pops (and employment where a head moved)",
            place_after.id
        );
    }
}

#[test]
fn shipped_network_joins_the_new_paths_to_their_roads() {
    let (_dir, prepared, _) = shipped(7);
    let network = prepared.scenario.network.as_ref().expect("shipped network");
    for (child, place, parent, old) in [
        ("path-taghmon", "taghmon", "chualann-2", "st_mullins"),
        ("path-ardmore", "ardmore", "munster-2", "cloyne"),
    ] {
        let link = network.links.iter().find(|l| l.id == child).unwrap();
        let road = network.links.iter().find(|l| l.id == parent).unwrap();
        let place_node = network
            .nodes
            .iter()
            .find(|n| n.place.as_deref() == Some(place))
            .unwrap();
        let old_node = network
            .nodes
            .iter()
            .find(|n| n.place.as_deref() == Some(old))
            .unwrap();
        let join = network.joins.iter().find(|j| j.link == child).unwrap();
        assert_eq!(link.from_node, place_node.id);
        assert_ne!(link.to_node, old_node.id);
        assert_eq!(join.parent, parent);
        assert_eq!(join.node, link.to_node);
        assert!(!link.stretches.is_empty());
        assert!(road.stretches.iter().any(|(id, _)| {
            let s = network.stretches.iter().find(|s| s.id == *id).unwrap();
            s.from == join.node || s.to == join.node
        }));
    }
}

#[test]
fn weather_is_present_for_every_shipped_place() {
    let (_dir, prepared, _) = shipped(1);
    let view = crate::game::setup::view_builder(prepared.view_wiring())(
        prepared.simulation.state(),
        prepared.simulation.date().unwrap(),
    );
    assert_eq!(view.places.len(), prepared.scenario.locations.len());
    assert!(view.places.iter().all(|place| place.temperature.is_some()));
}

#[test]
fn a_changed_weather_rule_refuses_an_existing_save() {
    let dir = Dir::new();
    let scenario = assets_with_scenario(&dir, "ireland-768");
    let game = GameConfig {
        scenario: scenario.clone(),
        saves: dir.0.join("saves"),
    };
    let (save, _) = super::setup::write_save(&game, 0, 42);
    let weather = scenario
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("game/weather.toml");
    let source = fs::read_to_string(&weather).unwrap();
    fs::write(
        weather,
        source.replace("swing_coast = 48", "swing_coast = 49"),
    )
    .unwrap();
    let error = prepare(
        &Request {
            game: &game,
            vms: &[],
            load: Some(&save),
            seed: None,
        },
        || Ok(0),
        &mut |_, _| {},
    )
    .err()
    .unwrap();
    assert!(error.contains("rules_hash"), "{error}");
}
