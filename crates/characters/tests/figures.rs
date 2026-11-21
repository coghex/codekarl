//! The loader of the real figures (CK-370 §1): the shipped files, each field read from the file that owns it, and every named refusal.

use ck_characters::{
    Office, PopLocator, Sex,
    figures::{FigureData, FigureError, HouseName, Houses, PopKind, Range},
};
use ck_economy::events::ROLES;

fn read(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenarios/ireland-768")
            .join(name),
    )
    .expect("a shipped scenario file")
}
fn files() -> (String, String) {
    (read("ireland-768.toml"), read("ireland-768-load.toml"))
}
fn shipped() -> FigureData {
    let (research, load) = files();
    FigureData::parse(&research, &load).expect("the shipped files load")
}
/// The shipped load file with its first `from` replaced by `to`.
fn load_with(from: &str, to: &str) -> Result<FigureData, FigureError> {
    let (research, load) = files();
    assert!(load.contains(from), "{from} is in the load file");
    FigureData::parse(&research, &load.replacen(from, to, 1))
}
fn research_with(from: &str, to: &str) -> Result<FigureData, FigureError> {
    let (research, load) = files();
    assert!(research.contains(from), "{from} is in the research file");
    FigureData::parse(&research.replacen(from, to, 1), &load)
}

#[test]
fn the_shipped_files_load_with_their_counts() {
    let data = shipped();
    assert_eq!(data.figures.len(), 282);
    assert_eq!(
        data.figures.iter().filter(|f| f.alive_at_start).count(),
        276
    );
    assert_eq!(data.dead_parents.len(), 10);
    assert_eq!(data.strands.len(), 16);
    assert_eq!(data.slots.len(), 118);
    assert_eq!(
        data.slots
            .iter()
            .map(|slot| slot.members.len())
            .sum::<usize>(),
        498
    );
    // `[defaults].excluded`: the nine traits, `fool:S` from grade S (4).
    assert_eq!(data.excluded.len(), 9);
    assert!(data.excluded.contains(&("fool".to_owned(), 4)));
    assert!(data.excluded.contains(&("blind".to_owned(), 1)));
}

#[test]
fn niall_frossach_takes_his_location_from_the_research_file_and_his_house_office_and_dates_from_the_load_file()
 {
    let data = shipped();
    let niall = data.figure("niall_frossach").expect("Niall Frossach");
    // The research file.
    assert_eq!(niall.location, "ailech");
    assert_eq!(
        (niall.name.as_str(), niall.genitive.as_str()),
        ("Niall", "Néill")
    );
    assert_eq!(niall.byname, "Frossach");
    assert_eq!(niall.patronymic, "mac Fergaile");
    assert_eq!(niall.sex, Sex::Male);
    assert_eq!(niall.mother_tongue, "old_irish");
    assert!(niall.second_languages.is_empty() && niall.forced.is_empty());
    assert!(!niall.beats.is_empty(), "the beats are kept, and inert");
    // The load file.
    assert_eq!(niall.house, "cenel_neogain");
    assert_eq!(niall.office, Some(Office::Ruler));
    assert_eq!(niall.pop_kind, PopKind::Noble);
    assert_eq!(
        (niall.father.as_str(), niall.mother.as_str()),
        ("fergal_mac_maele_duin", "aithiochta")
    );
    assert_eq!(niall.spouse, "dunlaith");
    assert_eq!((niall.born.year, niall.born.estimated), (718, false));
    assert_eq!(niall.died.map(|died| died.year), Some(778));
    assert!(niall.alive_at_start);
    // The prose `born` and `died` of the research file are not read: they would be `"735 est"` for Dúnlaith.
    let dunlaith = data.figure("dunlaith").expect("Dúnlaith");
    assert_eq!((dunlaith.born.year, dunlaith.born.estimated), (735, true));
    assert_eq!(dunlaith.house_of_birth, "cenel_conaill");
}

#[test]
fn a_language_range_is_read_exactly_in_thousandths() {
    let data = shipped();
    let ranges: Vec<Range> = data
        .figures
        .iter()
        .flat_map(|figure| figure.second_languages.iter())
        .filter(|(language, _)| language == "latin")
        .map(|(_, range)| *range)
        .collect();
    assert!(
        ranges.contains(&Range {
            low: 600,
            high: 950
        }),
        "{ranges:?}"
    );
    assert!(ranges.contains(&Range { low: 0, high: 300 }));
    assert!(ranges.contains(&Range {
        low: 200,
        high: 600
    }));
}

#[test]
fn the_two_new_roles_are_in_the_registry_and_in_the_shipped_slots() {
    assert_eq!(ROLES.len(), 29);
    let data = shipped();
    for role in ["household_woman", "court_cleric"] {
        assert!(ROLES.contains(&role));
        assert!(
            data.slots
                .iter()
                .flat_map(|slot| slot.members.iter())
                .any(|member| member.role == role),
            "{role} is a role of a shipped slot"
        );
    }
    // Every role of every slot is in the registry (the loader refuses one that is not).
    assert!(
        data.slots
            .iter()
            .flat_map(|slot| slot.members.iter())
            .all(|member| ROLES.contains(&member.role.as_str()))
    );
}

#[test]
fn a_figure_dated_before_the_start_and_the_estimates_are_read_as_dates() {
    let data = shipped();
    let fer = data.figure("fer_da_chrich").expect("Fer dá Chrích");
    assert!(!fer.alive_at_start);
    assert_eq!(fer.died_date, Some((768, 5, 18)));
    assert!(fer.born.estimated);
    let suibne = data
        .dead_parents
        .iter()
        .find(|p| p.key == "suibne")
        .expect("Suibne");
    assert_eq!(suibne.died, None, "a dead parent with no recorded death");
    let fergal = data
        .dead_parents
        .iter()
        .find(|p| p.key == "fergal_mac_maele_duin")
        .expect("Fergal");
    assert_eq!(fergal.died, Some(722));
}

#[test]
fn every_named_refusal_names_what_it_refused() {
    // A load-file key that is not a figure or a dead parent.
    assert!(matches!(
        load_with("father = \"fergal_mac_maele_duin\"", "father = \"nobody\""),
        Err(FigureError::UnknownKey { key, .. }) if key == "nobody"
    ));
    // A figure the research file has and the load file does not.
    let (research, load) = files();
    let cut = load.find("[[figure]]").expect("a figure");
    let cut_end = cut + load[cut..].find("\n\n").expect("a blank line");
    let without = format!("{}{}", &load[..cut], &load[cut_end..]);
    assert!(matches!(
        FigureData::parse(&research, &without),
        Err(FigureError::UnknownKey { .. })
    ));
    // An office other than the two.
    assert!(matches!(
        load_with("office = \"ruler\"", "office = \"heir\""),
        Err(FigureError::Office { office, .. }) if office == "heir"
    ));
    // A role outside the registry.
    assert!(matches!(
        load_with("role = \"retainer\"", "role = \"squire\""),
        Err(FigureError::Role { role, .. }) if role == "squire"
    ));
    // A strand kind that is not slice 6's.
    assert!(matches!(
        load_with("kind = \"rival\"", "kind = \"enemy\""),
        Err(FigureError::StrandKind { kind, .. }) if kind == "enemy"
    ));
    // A father who is a woman, and a mother who is a man.
    assert!(matches!(
        load_with("father = \"fergal_mac_maele_duin\"", "father = \"dunlaith\""),
        Err(FigureError::ParentSex { parent, .. }) if parent == "dunlaith"
    ));
    assert!(matches!(
        load_with("mother = \"aithiochta\"", "mother = \"niall_frossach\""),
        Err(FigureError::ParentSex { .. })
    ));
    // A cycle in the parent keys: Niall's father is his own son.
    assert!(matches!(
        load_with(
            "father = \"fergal_mac_maele_duin\"",
            "father = \"aed_oirdnide\""
        ),
        Err(FigureError::ParentCycle { .. })
    ));
    // A key used twice.
    let (research, load) = files();
    let twice = format!("{load}\n[[figure]]\nkey = \"niall_frossach\"\n");
    assert!(matches!(
        FigureData::parse(&research, &twice),
        Err(FigureError::DuplicateKey { key, .. }) if key == "niall_frossach"
    ));
    // A trait that is given and excluded.
    assert!(matches!(
        research_with("forced = []", "forced = [\"blind\"]"),
        Err(FigureError::ForcedExcluded { key, .. }) if key == "blind"
    ));
    // A living figure with a death before the start.
    assert!(matches!(
        load_with(
            "alive_at_start = true",
            "alive_at_start = true\ndied_before_start = true"
        ),
        Err(FigureError::Dates { .. })
    ));
    // A file that is not TOML.
    assert!(matches!(
        FigureData::parse("not = [", &files().1),
        Err(FigureError::Syntax { .. })
    ));
}

#[test]
fn two_children_of_one_mother_born_in_one_year_are_twins_and_load() {
    // Births carry a year alone, so a year shared is twins: the nine-month rule refuses nothing in whole years.
    let (research, load) = files();
    let twins = load.replacen("born = 756", "born = 754", 1);
    assert_ne!(twins, load);
    FigureData::parse(&research, &twins).expect("twins load");
}

#[test]
fn the_places_and_pops_of_the_figures_and_slots_are_joined_to_the_scenarios() {
    let data = shipped();
    let pop = |place: &str, slot: u32, kind: &str| {
        (
            PopLocator {
                place: place.to_owned(),
                slot,
            },
            kind.to_owned(),
            "mumu".to_owned(),
        )
    };
    // Every place and pop the data asks for, once.
    let mut all: Vec<(PopLocator, String, String)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut wanted = |place: &str, kind: &str| {
        if seen.insert((place.to_owned(), kind.to_owned())) {
            let slot = u32::try_from(all.len()).expect("a slot");
            all.push(pop(place, slot, kind));
        }
    };
    for figure in &data.figures {
        wanted(&figure.location, figure.pop_kind.key());
    }
    for parent in &data.dead_parents {
        wanted(&parent.location, parent.pop_kind.key());
    }
    for slot in &data.slots {
        for member in &slot.members {
            wanted(&slot.location, &member.pop);
        }
    }
    data.check_pops(&all).expect("every pop exists");
    // A place that does not exist, and a place without the pop type.
    let missing_place: Vec<_> = all
        .iter()
        .filter(|(p, _, _)| p.place != "ailech")
        .cloned()
        .collect();
    assert!(matches!(
        data.check_pops(&missing_place),
        Err(FigureError::Pop { kind, .. }) if kind.is_empty()
    ));
    let no_monks: Vec<_> = all
        .iter()
        .filter(|(_, kind, _)| kind != "monk")
        .cloned()
        .collect();
    assert!(matches!(
        data.check_pops(&no_monks),
        Err(FigureError::Pop { kind, .. }) if kind == "monk"
    ));
}

#[test]
fn a_living_figure_may_carry_a_later_death_as_history_but_not_one_before_the_start() {
    // Niall is alive at the start and dies in 778: a death date after the start is history, and the figure stays live.
    let data = load_with(
        "alive_at_start = true",
        "alive_at_start = true\ndied_date = \"0778-03-04\"",
    )
    .expect("a later death date is history");
    assert!(data.figure("niall_frossach").expect("Niall").alive_at_start);
    assert_eq!(
        data.figure("niall_frossach").expect("Niall").died_date,
        Some((778, 3, 4))
    );
    // A death date before the start contradicts `alive_at_start`.
    assert!(matches!(
        load_with(
            "alive_at_start = true",
            "alive_at_start = true\ndied_date = \"0768-05-18\""
        ),
        Err(FigureError::Dates { .. })
    ));
}

#[test]
fn a_date_that_is_not_a_day_of_the_calendar_is_a_named_error() {
    // 29 February exists only in a leap year, and 30 February never.
    assert!(
        matches!(
            load_with(
                "alive_at_start = true",
                "alive_at_start = true\ndied_date = \"0778-02-29\""
            ),
            Err(FigureError::Date { .. })
        ),
        "778 is not a leap year"
    );
    for bad in [
        "0778-02-30",
        "0778-13-01",
        "0778-00-10",
        "0778-04-31",
        "0778-04-00",
        "778-4",
    ] {
        let result = load_with(
            "alive_at_start = true",
            &format!("alive_at_start = true\ndied_date = \"{bad}\""),
        );
        assert!(
            matches!(result, Err(FigureError::Date { ref value, .. }) if value == bad),
            "{bad}: {result:?}"
        );
    }
    // 29 February of a leap year is valid.
    load_with(
        "alive_at_start = true",
        "alive_at_start = true\ndied_date = \"0780-02-29\"",
    )
    .expect("a leap day");
}

#[test]
fn a_trait_is_refused_only_at_a_level_its_exclusion_forbids() {
    // `poet` at C (1) beside `poet:B` (2) is allowed; at B or above it is not.
    research_with(
        "forced = []",
        "forced = [\"poet\"]\nexcluded = [\"poet:B\"]",
    )
    .expect("poet at C is allowed");
    assert!(matches!(
        research_with("forced = []", "forced = [\"poet:A\"]\nexcluded = [\"poet:B\"]"),
        Err(FigureError::ForcedExcluded { key, .. }) if key == "poet"
    ));
}

#[test]
fn a_figures_own_exclusions_add_to_the_defaults_and_never_replace_them() {
    let data = research_with(
        "forced = []",
        "forced = []\nexcluded = [\"stammer\", \"blind:B\"]",
    )
    .expect("loads");
    let niall = data.figure("niall_frossach").expect("Niall");
    // The defaults stay (nine traits), the figure's own add, and a trait in both keeps the stricter level.
    assert!(niall.excluded.contains(&("madness".to_owned(), 1)));
    assert!(niall.excluded.contains(&("fool".to_owned(), 4)));
    assert!(niall.excluded.contains(&("stammer".to_owned(), 1)));
    assert!(
        niall.excluded.contains(&("blind".to_owned(), 1)),
        "the default's level 1 is stricter than `blind:B` (2)"
    );
    assert_eq!(
        niall
            .excluded
            .iter()
            .filter(|(key, _)| key == "blind")
            .count(),
        1
    );
    assert_eq!(niall.excluded.len(), 10);
}

fn houses_text() -> String {
    read("houses.toml")
}

#[test]
fn the_shipped_house_names_load_and_name_every_house_the_load_file_uses() {
    let data = shipped()
        .with_houses(&houses_text())
        .expect("the houses join");
    assert_eq!(data.houses.houses.len(), 35);
    assert_eq!(data.houses.subhouses.len(), 5);
    let niall = data.figure("niall_frossach").expect("Niall");
    let named = &data.houses.houses[&niall.house];
    assert_eq!(named.line(), "Uí Néill · Cenél nEógain");
    // A house with no dynasty shows its name alone.
    assert_eq!(data.houses.houses["ui_maine"].line(), "Uí Maine");
    assert_eq!(data.houses.subhouses["sil_cathail"].house, "ui_briuin");
    // The same file is read for a loaded game, with no figures to check it against.
    assert_eq!(Houses::parse(&houses_text()).expect("parses"), data.houses);
}

#[test]
fn a_house_key_the_houses_file_does_not_name_is_a_load_error_but_the_empty_house_is_not() {
    let houses = houses_text();
    // A load-file house the file lacks.
    let (research, load) = files();
    let renamed = load.replacen("house = \"cenel_neogain\"", "house = \"cenel_nowhere\"", 1);
    assert_ne!(renamed, load);
    let error = FigureData::parse(&research, &renamed)
        .expect("parses by itself")
        .with_houses(&houses)
        .expect_err("refused");
    assert!(
        matches!(&error, FigureError::UnknownKey { key, .. } if key == "cenel_nowhere"),
        "{error}"
    );
    // A subhouse the file lacks, and a house of birth.
    let subhouse = load.replacen(
        "subhouse = \"sil_cathail\"",
        "subhouse = \"sil_nowhere\"",
        1,
    );
    assert!(
        FigureData::parse(&research, &subhouse)
            .expect("parses")
            .with_houses(&houses)
            .is_err()
    );
    // The empty house needs no name: the shipped load file has figures with none.
    let data = shipped().with_houses(&houses).expect("joins");
    assert!(data.figures.iter().any(|figure| figure.house.is_empty()));
}

#[test]
fn a_malformed_houses_file_is_refused_by_name() {
    for (what, text) in [
        (
            "an unknown table",
            "[houses]\na = { name = \"A\" }\n".to_owned(),
        ),
        (
            "an unknown field",
            "[house]\na = { name = \"A\", rank = \"high\" }\n".to_owned(),
        ),
        ("no name", "[house]\na = { dynasty = \"D\" }\n".to_owned()),
        ("an empty name", "[house]\na = { name = \"\" }\n".to_owned()),
        (
            "a subhouse of a house the file does not name",
            "[house]\na = { name = \"A\" }\n[subhouse]\ns = { name = \"S\", house = \"b\" }\n"
                .to_owned(),
        ),
        ("not toml", "[house".to_owned()),
    ] {
        assert!(Houses::parse(&text).is_err(), "{what}");
    }
    let ok =
        Houses::parse("[house]\na = { name = \"A\", dynasty = \"D\" }\nb = { name = \"B\" }\n")
            .expect("parses");
    assert_eq!(
        ok.houses["a"],
        HouseName {
            name: "A".to_owned(),
            dynasty: "D".to_owned()
        }
    );
    assert_eq!(ok.houses["b"].line(), "B");
}
