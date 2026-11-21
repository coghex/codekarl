//! The 768 start on a small world (CK-370 §3): figures with their fixed aspects, dead parents and archived figures, ancestry, households, slots, the
//! relatives of clergy, the safety net, the burn-in, the known strands and determinism. The shipped scenario runs in the application's tests.

#![expect(
    clippy::format_push_string,
    reason = "a test fixture builds its two files as text, a line at a time"
)]

mod support;

use ck_characters::{
    Archive, ArchiveSink, Character, CharacterStore, Departure, Office, Populations, Sex, UNKNOWN,
    figures::FigureData,
    start::{StartError, StartReport},
};
use ck_sim::{Canonical, GameTime};
use support::{Env, Pops, date, locator, shipped, without_relative_deaths, world};

fn start_day() -> GameTime {
    date(768, 10, 9)
}

/// One figure of the fixture, with the fields both files hold.
#[derive(Clone)]
struct Fig {
    key: &'static str,
    name: &'static str,
    sex: &'static str,
    place: &'static str,
    kind: &'static str,
    born: u16,
    house: &'static str,
    house_of_birth: &'static str,
    office: &'static str,
    father: &'static str,
    mother: &'static str,
    spouse: &'static str,
    byname: &'static str,
    genitive: &'static str,
    patronymic: &'static str,
    tongue: &'static str,
    latin: bool,
    forced: &'static str,
    alive: bool,
    died: Option<u16>,
    died_date: &'static str,
    before_start: bool,
}

fn fig(
    key: &'static str,
    place: &'static str,
    kind: &'static str,
    sex: &'static str,
    born: u16,
) -> Fig {
    Fig {
        key,
        name: key,
        sex,
        place,
        kind,
        born,
        house: "",
        house_of_birth: "",
        office: "",
        father: "",
        mother: "",
        spouse: "",
        byname: "",
        genitive: "",
        patronymic: "",
        tongue: "old_irish",
        latin: false,
        forced: "",
        alive: true,
        died: None,
        died_date: "",
        before_start: false,
    }
}

/// A dead parent: key, sex, born, died (none for unknown), house, kind and place.
type Dead = (
    &'static str,
    &'static str,
    u16,
    Option<u16>,
    &'static str,
    &'static str,
    &'static str,
);

/// The two files of a fixture.
fn files(
    figures: &[Fig],
    dead: &[Dead],
    strands: &[(&str, &str, &str, u8)],
    slots: &str,
    excluded: &str,
) -> (String, String) {
    let mut research = format!("[defaults]\nexcluded = [{excluded}]\n\n");
    let mut load = String::new();
    for f in figures {
        research += &format!(
            "[[figure]]\nkey = \"{}\"\nname = \"{}\"\nsex = \"{}\"\nlocation = \"{}\"\nmother_tongue = \"{}\"\nforced = [{}]\n",
            f.key, f.name, f.sex, f.place, f.tongue, f.forced
        );
        if f.latin {
            research += "second_languages = { latin = [0.6, 0.95] }\n";
        } else {
            research += "second_languages = {}\n";
        }
        if !f.byname.is_empty() {
            research += &format!("byname = \"{}\"\n", f.byname);
        }
        for (field, value) in [("genitive", f.genitive), ("patronymic", f.patronymic)] {
            if !value.is_empty() {
                research += &format!("{field} = \"{value}\"\n");
            }
        }
        research += "\n";
        load += &format!(
            "[[figure]]\nkey = \"{}\"\nborn = {}\nborn_estimated = true\n",
            f.key, f.born
        );
        for (field, value) in [
            ("house", f.house),
            ("house_of_birth", f.house_of_birth),
            ("office", f.office),
            ("father", f.father),
            ("mother", f.mother),
            ("spouse", f.spouse),
            ("died_date", f.died_date),
        ] {
            if !value.is_empty() {
                load += &format!("{field} = \"{value}\"\n");
            }
        }
        if let Some(died) = f.died {
            load += &format!("died = {died}\n");
        }
        if f.before_start {
            load += "died_before_start = true\n";
        }
        load += &format!(
            "alive_at_start = {}\npop_kind = \"{}\"\n\n",
            f.alive,
            if f.kind == "monk" { "monk" } else { "noble" }
        );
    }
    for (key, sex, born, died, house, kind, place) in dead {
        load += &format!(
            "[[dead_parent]]\nkey = \"{key}\"\nname = \"{key}\"\ngenitive = \"{key}s\"\nsex = \"{sex}\"\nborn = {born}\nhouse = \"{house}\"\npop_kind = \"{kind}\"\nlocation = \"{place}\"\n"
        );
        if let Some(died) = died {
            load += &format!("died = {died}\n");
        }
        load += "\n";
    }
    for (a, b, kind, strength) in strands {
        load += &format!(
            "[[strand]]\na = \"{a}\"\nb = \"{b}\"\nkind = \"{kind}\"\nstrength = {strength}\n\n"
        );
    }
    load += slots;
    (research, load)
}

/// The slots of the fixture: a court with the king's household, a lord's court with a generated wife, a church community with students, and a
/// small seat whose ruler has no wife.
const SLOTS: &str = r#"
[[slot]]
key = "court_slot"
location = "court"
[[slot.member]]
role = "spouse_of_ruler"
count = 1
pop = "noble"
sex = "f"
[[slot.member]]
role = "household_woman"
count = 2
pop = "noble"
sex = "f"
[[slot.member]]
role = "court_cleric"
count = 1
pop = "noble"
sex = "m"
[[slot.member]]
role = "retainer"
count = 2
pop = "noble"
sex = "m"
[[slot.member]]
role = "judge"
count = 1
pop = "noble"
sex = "m"
[[slot.member]]
role = "poet"
count = 1
pop = "noble"
sex = "m"
[[slot.member]]
role = "herdsman"
count = 1
pop = "noble"
sex = "m"

[[slot]]
key = "lord_slot"
location = "court"
[[slot.member]]
role = "spouse_of_lord"
count = 1
pop = "noble"
sex = "f"
[[slot.member]]
role = "retainer"
count = 1
pop = "noble"
sex = "m"
[[slot.member]]
role = "lord"
count = 1
pop = "noble"
sex = "m"

[[slot]]
key = "church_slot"
location = "church"
[[slot.member]]
role = "scribe"
count = 2
pop = "monk"
sex = "m"
[[slot.member]]
role = "student"
count = 2
pop = "monk"
sex = "m"
from_noble_families = true

[[slot]]
key = "fost_m"
location = "court"
[[slot.member]]
role = "fosterling"
count = 3
pop = "noble"
sex = "m"

[[slot]]
key = "fost_f"
location = "court"
[[slot.member]]
role = "fosterling"
count = 3
pop = "noble"
sex = "f"

[[slot]]
key = "spouse_again"
location = "court"
[[slot.member]]
role = "spouse_of_ruler"
count = 1
pop = "noble"
sex = "f"

[[slot]]
key = "fort_slot"
location = "fort"
[[slot.member]]
role = "spouse_of_ruler"
count = 1
pop = "noble"
sex = "f"
"#;

/// The figures of the fixture.
fn figures() -> Vec<Fig> {
    let mut all = vec![
        Fig {
            name: "Zzking",
            house: "ui_x",
            office: "ruler",
            spouse: "queen",
            byname: "Oirdnide",
            ..fig("king", "court", "noble", "m", 720)
        },
        Fig {
            name: "Zzqueen",
            house: "ui_x",
            house_of_birth: "ui_y",
            father: "ui_y_dad",
            spouse: "king",
            ..fig("queen", "court", "noble", "f", 730)
        },
        Fig {
            name: "Zzprince",
            house: "ui_x",
            father: "king",
            mother: "queen",
            ..fig("prince", "court", "noble", "m", 760)
        },
        Fig {
            name: "Zzeithne",
            house: "ui_x",
            spouse: "king",
            alive: false,
            died: Some(768),
            ..fig("eithne", "court", "noble", "f", 735)
        },
        Fig {
            name: "Zzmay",
            house: "ui_z",
            alive: false,
            died_date: "0768-05-18",
            died: Some(768),
            ..fig("died_may", "court", "noble", "m", 700)
        },
        Fig {
            name: "Zzunknown",
            house: "ui_z",
            alive: false,
            before_start: true,
            ..fig("unknown_end", "court", "noble", "m", 690)
        },
        Fig {
            name: "Zzabbot",
            office: "high_cleric",
            father: "suibne",
            byname: "Odar",
            latin: true,
            ..fig("abbot", "church", "monk", "m", 700)
        },
        Fig {
            name: "Zzking2",
            house: "ui_w",
            office: "ruler",
            ..fig("king2", "fort", "noble", "m", 738)
        },
    ];
    for (n, born) in (750..756).enumerate() {
        let (isle, english): (&'static str, &'static str) = [
            ("isle1", "eng1"),
            ("isle2", "eng2"),
            ("isle3", "eng3"),
            ("isle4", "eng4"),
            ("isle5", "eng5"),
            ("isle6", "eng6"),
        ][n];
        all.push(Fig {
            name: isle,
            ..fig(isle, "isle", "monk", "m", born)
        });
        all.push(Fig {
            name: english,
            tongue: "old_english",
            ..fig(english, "mayo", "monk", "m", born)
        });
    }
    for (n, born) in [750_u16, 751].into_iter().enumerate() {
        let key = ["hermit1", "hermit2"][n];
        all.push(Fig {
            name: key,
            ..fig(key, "hermit", "monk", "m", born)
        });
    }
    all
}

/// The dead parents of the fixture.
fn dead_parents() -> Vec<Dead> {
    vec![
        ("ui_y_dad", "m", 700, Some(750), "ui_y", "noble", "court"),
        ("suibne", "m", 680, None, "", "monk", "church"),
    ]
}

/// The traits no figure of the fixture is given: three of the shipped list, and two common enough to show.
const EXCLUDED: &str = "\"blind\", \"madness\", \"fool:S\", \"left_handed\", \"beauty\"";

fn data_with(excluded: &str, figures: &[Fig]) -> FigureData {
    let (research, load) = files(
        figures,
        &dead_parents(),
        &[
            ("abbot", "isle1", "rival", 2),
            ("died_may", "abbot", "rival", 3),
        ],
        SLOTS,
        excluded,
    );
    FigureData::parse(&research, &load).expect("the fixture loads")
}

fn data() -> FigureData {
    data_with(EXCLUDED, &figures())
}

/// The pops of the fixture: a court and a small seat of nobles; a church with peasants; a hermitage with labourers and no peasants; an isle of monks
/// alone, near the church and a farm; and a Northumbrian monastery of a culture no other pop has.
fn pops(court_heads: u64) -> Pops {
    Pops::new()
        .pop("court", 0, "noble", "mumu", court_heads)
        .pop("court", 1, "labourer", "mumu", 30)
        .pop("fort", 0, "noble", "mumu", 60)
        .pop("church", 0, "monk", "mumu", 40)
        .pop("church", 1, "peasant", "mumu", 100_000)
        .pop("hermit", 0, "monk", "mumu", 5)
        .pop("hermit", 1, "labourer", "mumu", 40)
        .pop("isle", 0, "monk", "mumu", 12)
        .pop("farm", 0, "peasant", "mumu", 100_000)
        .pop("mayo", 0, "monk", "northumbrian", 20)
        .pop("crag", 0, "noble", "mumu", 200)
        .pop("crag", 1, "labourer", "mumu", 20)
        .pop("glen", 0, "noble", "mumu", 300)
        .pop("glen", 1, "peasant", "mumu", 100_000)
        .apart("church", "court", 30_000)
        .apart("fort", "court", 10_000)
        .apart("isle", "church", 20_000)
        .apart("isle", "farm", 40_000)
        .apart("church", "farm", 25_000)
        .apart("hermit", "farm", 70_000)
        .apart("crag", "farm", 25_000)
        .apart("crag", "church", 45_000)
}

/// A finished start.
struct Started {
    store: CharacterStore,
    env: Env,
    pops: Pops,
    report: StartReport,
}

fn started(tag: &str, seed: u64, court_heads: u64, net: bool) -> Result<Started, StartError> {
    started_on(tag, seed, court_heads, net, &data())
}

fn started_on(
    tag: &str,
    seed: u64,
    court_heads: u64,
    net: bool,
    data: &FigureData,
) -> Result<Started, StartError> {
    started_with(
        tag,
        seed,
        court_heads,
        net,
        data,
        (shipped(), &support::rules().events),
    )
}

/// A start on `data` under the life-cycle and event data given.
fn started_with(
    tag: &str,
    seed: u64,
    court_heads: u64,
    net: bool,
    data: &FigureData,
    (lifecycle, events): (
        &'static ck_economy::LifeCycle,
        &'static ck_economy::EventRules,
    ),
) -> Result<Started, StartError> {
    let pops = pops(court_heads);
    pops.net.set(net);
    let mut env = Env::new(tag);
    let mut store = CharacterStore::default();
    let report = {
        let mut w = support::world_with(events, lifecycle, &mut env, &pops, seed);
        store.start_768(&mut w, data, start_day())?
    };
    Ok(Started {
        store,
        env,
        pops,
        report,
    })
}

fn go(tag: &str) -> Started {
    started(tag, 1, 400, false).expect("the start")
}

/// The record of the character called `name`, live or archived.
fn named(s: &Started, name: &str) -> Character {
    if let Some(live) = s.store.iter().find(|c| c.name == name) {
        return live.clone();
    }
    s.env
        .archived_ids()
        .into_iter()
        .filter_map(|id| s.env.archive.get(id).expect("readable"))
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no character called {name}"))
}

fn live(s: &Started, name: &str) -> Character {
    s.store
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no live character called {name}"))
        .clone()
}

fn year_of(time: GameTime) -> u16 {
    time.date().expect("a date").year
}

#[test]
fn a_figure_is_made_in_its_pop_with_its_house_office_name_byname_and_languages() {
    let s = go("fixed");
    let king = live(&s, "Zzking");
    assert_eq!(king.pop, locator("court", 0));
    assert_eq!(king.sex, Sex::Male);
    assert_eq!(king.birth, date(720, 1, 1), "a year alone is 1 January");
    assert_eq!(king.life.house, "ui_x");
    assert_eq!(king.office, Some(Office::Ruler));
    // The byname matches the Irish model `oirdnide` of epithets.toml, whose importance it takes; the other has none, so 40.
    assert_eq!(king.life.byname, "Oirdnide");
    assert_eq!(king.life.byname_importance, 45);
    let abbot = live(&s, "Zzabbot");
    assert_eq!(abbot.life.byname, "Odar");
    assert_eq!(abbot.life.byname_importance, 40);
    assert_eq!(abbot.office, Some(Office::HighCleric));
    assert_eq!(abbot.pop, locator("church", 0));
    // The shown name carries the byname.
    let father = s.store.get(king.father).cloned();
    let shown = ck_characters::cycle::shown_name(&king, father.as_ref(), shipped());
    assert!(shown.ends_with("Oirdnide"), "{shown}");
    // Languages: the mother tongue is the data's, the second languages exactly the ones it names, each in its range and rolled once.
    assert_eq!(king.life.tongue, "old_irish");
    assert!(king.life.languages.is_empty());
    assert_eq!(abbot.life.tongue, "old_irish");
    let latin = abbot.life.languages.get("latin").copied().expect("Latin");
    assert!((600..=950).contains(&latin), "{latin}");
    assert_eq!(abbot.life.languages.len(), 1);
    let english = live(&s, "eng1");
    assert_eq!(english.life.tongue, "old_english");
    assert_eq!(english.life.dialect, "northumbrian");
    // The excluded traits are never given to a real figure: blindness and madness at any grade, a fool at grade S (4) or above.
    for name in [
        "Zzking", "Zzqueen", "Zzprince", "Zzabbot", "Zzking2", "eng1", "isle1", "hermit1",
    ] {
        let c = live(&s, name);
        assert_eq!(c.grade("blind"), 0, "{name}");
        assert_eq!(c.grade("madness"), 0, "{name}");
        assert!(c.grade("fool") < 4, "{name}");
    }
}

#[test]
fn an_excluded_trait_is_never_given_to_a_real_figure_and_a_forced_one_is() {
    let names = [
        "Zzking", "Zzqueen", "Zzprince", "Zzabbot", "Zzking2", "isle1", "isle2", "isle3", "isle4",
        "isle5", "isle6", "hermit1", "hermit2",
    ];
    let shown = |s: &Started| {
        names
            .iter()
            .filter(|name| {
                let c = live(s, name);
                c.grade("beauty") > 0 || c.grade("left_handed") > 0
            })
            .count()
    };
    // Without exclusions, common traits show among the figures: the test has something to keep off.
    let open = started_on("open", 1, 400, false, &data_with("", &figures())).expect("a start");
    assert!(
        shown(&open) > 0,
        "with nothing excluded some figure shows beauty or left-handedness"
    );
    // With them excluded, none does, however the roll and the generated years went.
    let closed = started_on("closed", 1, 400, false, &data()).expect("a start");
    assert_eq!(shown(&closed), 0);
    // A forced trait is the figure's: the abbot's stammer and the king's keen eye are given, whatever the recipe rolled.
    let mut forced = figures();
    for f in &mut forced {
        match f.key {
            "abbot" => f.forced = "\"stammer\"",
            "king" => f.forced = "\"keen_eyed\"",
            _ => {}
        }
    }
    let s = started_on("forced", 1, 400, false, &data_with(EXCLUDED, &forced)).expect("a start");
    assert!(live(&s, "Zzabbot").grade("stammer") >= 1);
    assert!(live(&s, "Zzking").grade("keen_eyed") >= 1);
}

#[test]
fn a_figure_dead_before_the_start_is_archived_with_its_dates_and_never_live() {
    let s = go("dead");
    assert!(
        s.store
            .iter()
            .all(|c| !["Zzmay", "Zzunknown", "Zzeithne"].contains(&c.name.as_str()))
    );
    // Died on 18 May 768: archived with that date.
    let may = named(&s, "Zzmay");
    assert_eq!(may.death, Some(date(768, 5, 18)));
    assert_eq!(may.life.departure, Departure::None);
    assert!(may.archivable());
    // Died before the start in a year nobody recorded: archived with an unknown death.
    let unknown = named(&s, "Zzunknown");
    assert_eq!(unknown.death, None);
    assert_eq!(unknown.life.departure, Departure::DeathUnknown);
    assert!(unknown.archivable());
    // A year alone is 1 January.
    assert_eq!(named(&s, "Zzeithne").death, Some(date(768, 1, 1)));
    // The dead parent with no recorded death: archived as unknown, through a real reopen of the archive and the canonical encoding.
    let suibne = named(&s, "suibne");
    assert_eq!(suibne.life.departure, Departure::DeathUnknown);
    let reopened = Archive::open(s.env.archive_path()).expect("the archive reopens");
    let again = reopened.get(suibne.id).expect("readable").expect("held");
    assert_eq!(again, suibne);
    let bytes = again.to_bytes().expect("encodes");
    assert_eq!(Character::from_bytes(&bytes).expect("decodes"), again);
    // A dead parent with a death year is dated 1 January of it.
    assert_eq!(named(&s, "ui_y_dad").death, Some(date(750, 1, 1)));
    // No archived record took a head: the court's pop has no more heads taken than it has live characters.
    let court = locator("court", 0);
    assert!(s.store.pop_count(&court) as u64 <= s.pops.heads(&court).expect("the pop"));
}

#[test]
fn an_archive_state_is_one_of_a_death_an_unknown_death_or_abroad_and_not_two() {
    let s = go("states");
    let mut both = named(&s, "Zzmay");
    both.life.departure = Departure::Abroad;
    assert!(
        !both.archivable(),
        "a dated death and an abroad state are two"
    );
    let mut live_record = live(&s, "Zzking");
    assert!(!live_record.archivable() && live_record.is_live_form());
    live_record.life.departure = Departure::DeathUnknown;
    assert!(live_record.archivable() && !live_record.is_live_form());
    // The archive refuses a record that is two states and a live one, and accepts the three states once each.
    let mut env = Env::new("states2");
    assert!(env.archive.append(&both).is_err());
    let alive = live(&s, "Zzprince");
    assert!(env.archive.append(&alive).is_err());
    // An abroad record has no pop on the map: one that still names a pop of the economy is refused at append, and the empty place is accepted
    // and survives a reopen.
    let mut abroad = alive;
    abroad.life.departure = Departure::Abroad;
    assert!(
        !abroad.archivable(),
        "abroad with an on-map pop is not canonical"
    );
    assert!(env.archive.append(&abroad).is_err());
    abroad.pop = ck_characters::PopLocator {
        place: String::new(),
        slot: 0,
    };
    assert!(abroad.archivable());
    env.archive
        .append(&abroad)
        .expect("an abroad record with no pop is archivable");
    let reopened = Archive::open(env.archive_path()).expect("reopens");
    assert_eq!(reopened.get(abroad.id).expect("readable"), Some(abroad));
    // The public creation path gives the same form: a character generated as `End::Abroad` has no pop.
    let pops = pops(400);
    let mut made = Env::new("states3");
    let mut store = CharacterStore::default();
    let id = {
        let mut w = world(shipped(), &mut made, &pops, 1);
        let mut request = ck_characters::generator::GenerateRequest::new(
            locator("court", 0),
            Sex::Male,
            ck_characters::generator::Birth::On(date(740, 1, 1)),
            "far",
            start_day(),
        );
        request.end = Some(ck_characters::generator::End::Abroad);
        store.generate(&mut w, &request).expect("generated").id
    };
    let far = support::record_in(&store, &made.archive, id);
    assert_eq!(far.life.departure, Departure::Abroad);
    assert!(
        far.archivable() && far.pop.place.is_empty(),
        "{:?}",
        far.pop
    );
    assert!(store.get(id).is_none());
}

#[test]
fn a_named_living_parent_gives_its_lineage_and_a_missing_parent_is_generated_before_its_child() {
    let s = go("ancestry");
    let (king, queen, prince) = (
        live(&s, "Zzking"),
        live(&s, "Zzqueen"),
        live(&s, "Zzprince"),
    );
    assert_eq!((prince.father, prince.mother), (king.id, queen.id));
    assert_eq!(
        prince.genetics.paternal, king.genetics.paternal,
        "a son's line is his father's"
    );
    assert_eq!(
        prince.genetics.maternal, queen.genetics.maternal,
        "a child's mother line is its mother's"
    );
    assert!(king.children.contains(&prince.id) && queen.children.contains(&prince.id));
    // The king names no parents: they are generated, and made before him.
    assert_ne!(king.father, UNKNOWN);
    assert_ne!(king.mother, UNKNOWN);
    assert!(king.father < king.id && king.mother < king.id);
    let father = support::record(&s.store, &s.env, king.father);
    assert_eq!(father.sex, Sex::Male);
    assert!(father.children.contains(&king.id));
    // The queen's named father is the dead parent, made before her.
    let dad = named(&s, "ui_y_dad");
    assert_eq!(queen.father, dad.id);
    assert!(dad.id < queen.id);
    assert_eq!(
        dad.father, UNKNOWN,
        "a dead parent's own parents are the unknown character"
    );
    // The queen's house of birth is hers, and her house is her husband's.
    assert_eq!(queen.life.house_of_birth, "ui_y");
    assert_eq!(queen.life.house, "ui_x");
    // A figure born after 752 has no generated past.
    assert!(prince.birth > date(752, 12, 31));
}

#[test]
fn the_named_spouse_is_the_living_one_and_the_archived_wife_stays_archived() {
    let s = go("spouses");
    let (king, queen) = (live(&s, "Zzking"), live(&s, "Zzqueen"));
    let eithne = named(&s, "Zzeithne");
    assert!(king.spouses.contains(&queen.id) && queen.spouses.contains(&king.id));
    assert!(
        king.spouses.contains(&eithne.id),
        "the archived wife stays a link"
    );
    assert!(eithne.spouses.contains(&king.id));
    assert!(
        s.store.get(eithne.id).is_none(),
        "she is archived, not live"
    );
    // The household was completed once and the king has exactly one living wife.
    let living: Vec<_> = king
        .spouses
        .iter()
        .filter(|id| s.store.get(**id).is_some())
        .collect();
    assert_eq!(living, [&queen.id]);
    assert!(king.life.household_complete && queen.life.household_complete);
}

#[test]
fn a_spouse_place_is_filled_by_the_subjects_spouse_and_a_lord_without_one_gets_a_generated_wife() {
    let s = go("places");
    // `spouse_of_ruler` binds to the living ruler at the slot's location: the named queen, and no new woman.
    let queen = live(&s, "Zzqueen");
    assert_eq!(queen.life.slot_key, "court_slot");
    assert!(
        s.store
            .iter()
            .all(|c| c.life.role != "spouse_of_ruler" || c.id != queen.id),
        "the named queen keeps her own life"
    );
    let ruler_places = s
        .store
        .iter()
        .filter(|c| {
            c.life.slot_key == "court_slot"
                && c.sex == Sex::Female
                && c.spouses.contains(&live(&s, "Zzking").id)
        })
        .count();
    assert_eq!(ruler_places, 1, "one place, one woman");
    // The lord is generated; his wife is generated and married to him and fills the `spouse_of_lord` place.
    let lords: Vec<_> = s.store.iter().filter(|c| c.life.role == "lord").collect();
    assert_eq!(lords.len(), 1);
    let lord = lords[0];
    assert_eq!(lord.pop, locator("court", 0));
    let wife = lord
        .spouses
        .iter()
        .filter_map(|id| s.store.get(*id))
        .find(|spouse| spouse.life.role == "spouse_of_lord")
        .expect("the lord's generated wife");
    assert_eq!(wife.life.slot_key, "lord_slot");
    assert_eq!(wife.sex, Sex::Female);
    assert!(wife.spouses.contains(&lord.id));
    // A ruler with no wife named has one by the end: generated and married, or made by his household, and bound to the place either way.
    let king2 = live(&s, "Zzking2");
    let wife2 = king2
        .spouses
        .iter()
        .filter_map(|id| s.store.get(*id))
        .find(|spouse| spouse.life.slot_key == "fort_slot")
        .expect("the ruler's wife");
    assert!(wife2.spouses.contains(&king2.id));
}

#[test]
fn the_slot_members_are_made_in_the_order_lord_first_then_the_rest_and_the_spouse_places_last() {
    let s = go("order");
    let lord = s
        .store
        .iter()
        .find(|c| c.life.role == "lord")
        .expect("a lord");
    let retainer = s
        .store
        .iter()
        .filter(|c| c.life.role == "retainer" && c.life.slot_key.is_empty())
        .map(|c| c.id)
        .collect::<Vec<_>>();
    // Members of the `lord_slot` are generated: the lord before the slot's retainer (ids grow in the order they are made).
    let lord_retainer = s
        .store
        .iter()
        .filter(|c| c.life.role == "retainer")
        .map(|c| c.id)
        .filter(|id| *id > lord.id)
        .count();
    assert!(
        lord_retainer >= 1,
        "the retainer of the lord's slot follows the lord: {retainer:?}"
    );
    // The household women and the court cleric are in the noble pop of the court, with their roles.
    let women = s
        .store
        .iter()
        .filter(|c| c.life.role == "household_woman")
        .count();
    assert_eq!(women, 2);
    let clerics: Vec<_> = s
        .store
        .iter()
        .filter(|c| c.life.role == "court_cleric")
        .collect();
    assert_eq!(clerics.len(), 1);
    assert_eq!(clerics[0].pop, locator("court", 0));
    // The community's scribes are in the church's monk pop with their Latin.
    let scribes: Vec<_> = s.store.iter().filter(|c| c.life.role == "scribe").collect();
    assert_eq!(scribes.len(), 2);
    assert!(
        scribes
            .iter()
            .all(|c| c.pop == locator("church", 0) && c.life.languages.contains_key("latin"))
    );
}

#[test]
fn a_student_from_noble_families_is_taken_from_a_noble_pop_and_moved_to_the_school() {
    let s = go("students");
    let students: Vec<_> = s
        .store
        .iter()
        .filter(|c| c.life.slot_kind == "student")
        .collect();
    assert_eq!(students.len(), 2);
    for student in students {
        assert_eq!(student.life.slot_key, "church_slot");
        assert_eq!(student.life.slot_kind, "student");
        assert_eq!(
            student.pop,
            locator("church", 0),
            "moved to the school's monk pop"
        );
        // It keeps its parents, as a fosterling does.
        assert_ne!(student.father, UNKNOWN);
    }
    // Every head moved into the school's monk pop came from a noble pop: the students are of noble families, never of church or peasant ones.
    let into_school: Vec<_> = s
        .pops
        .moves
        .borrow()
        .iter()
        .filter(|(_, to)| *to == locator("church", 0))
        .cloned()
        .collect();
    assert!(!into_school.is_empty());
    for (from, _) in into_school {
        assert!(
            ["court", "fort"].contains(&from.place.as_str()) && from.slot == 0,
            "{from:?}"
        );
    }
    assert!(
        !s.pops.moves.borrow().is_empty(),
        "the students' heads moved with them"
    );
}

/// The ids of the generated parents of the figures named `names`.
fn parents_of(s: &Started, names: &[&str]) -> Vec<Character> {
    names
        .iter()
        .map(|name| named(s, name))
        .flat_map(|c| [c.father, c.mother])
        .filter(|id| *id != UNKNOWN)
        .map(|id| support::record(&s.store, &s.env, id))
        .collect()
}

#[test]
fn a_living_parent_of_a_monk_lives_in_the_places_peasant_pop_or_at_the_nearest_place_with_one() {
    let s = go("relatives");
    // The church has peasants of the monk's culture; the isle has none, so the nearest place with some (the church, 20 km, not the farm,
    // 40 km) takes them.
    let church = parents_of(&s, &["Zzabbot"]);
    let isle = parents_of(&s, &["isle1", "isle2", "isle3", "isle4", "isle5", "isle6"]);
    let mut living = 0;
    for parent in church.iter().chain(&isle) {
        if let Some(c) = s.store.get(parent.id) {
            living += 1;
            assert!(
                c.pop.place == "church" || c.pop.place == "farm",
                "{:?}",
                c.pop
            );
            assert_ne!(c.pop, locator("isle", 0), "never in the monk pop");
        }
    }
    assert!(living >= 3, "the monks' parents are mostly alive: {living}");
    for parent in &isle {
        if let Some(c) = s.store.get(parent.id) {
            assert_eq!(
                c.pop,
                locator("church", 1),
                "the nearest peasant pop of the culture"
            );
        }
    }
    // Its spouse and children, being relatives too, are outside the monk pop as well.
    for monk in ["Zzabbot", "isle1", "isle2", "isle3"] {
        let c = live(&s, monk);
        assert!(c.pop.place == "church" || c.pop.place == "isle");
        for relative in c
            .spouses
            .iter()
            .chain(&c.children)
            .filter_map(|id| s.store.get(*id))
        {
            assert_ne!(
                relative.pop, c.pop,
                "{monk}'s relative is not in the monk pop"
            );
        }
    }
}

#[test]
fn at_a_place_with_no_peasants_a_monks_living_parent_lives_at_the_nearest_peasant_pop_not_the_places_labourers()
 {
    let s = go("labourers");
    let parents = parents_of(&s, &["hermit1", "hermit2"]);
    let living: Vec<_> = parents.iter().filter_map(|p| s.store.get(p.id)).collect();
    assert!(!living.is_empty(), "a parent of a monk of 18 is alive");
    for c in living {
        assert_eq!(
            c.pop,
            locator("farm", 0),
            "the nearest peasant pop of the culture, not the hermitage's labourers"
        );
    }
    assert_eq!(
        s.store.pop_count(&locator("hermit", 0)),
        2,
        "the monk pop holds its two monks and no relative"
    );
    assert_eq!(
        s.store.pop_count(&locator("hermit", 1)),
        0,
        "the labourer pop holds only whom the research put there"
    );
}

/// A lay slot at a place with no peasants (`crag`) and one at a place with them (`glen`).
const LAY_SLOTS: &str = r#"
[[slot]]
key = "crag_slot"
location = "crag"
[[slot.member]]
role = "craftsman"
count = 4
pop = "labourer"
sex = "m"
[[slot.member]]
role = "retainer"
count = 3
pop = "noble"
sex = "m"

[[slot]]
key = "hermit_slot"
location = "hermit"
[[slot.member]]
role = "craftsman"
count = 3
pop = "labourer"
sex = "m"
[[slot.member]]
role = "monk"
count = 2
pop = "monk"
sex = "m"

[[slot]]
key = "glen_slot"
location = "glen"
[[slot.member]]
role = "retainer"
count = 6
pop = "noble"
sex = "m"
"#;

#[test]
fn at_a_monastic_place_every_relative_lives_at_the_nearest_peasant_pop_and_elsewhere_families_stay()
{
    let pops = pops(400);
    let mut env = Env::new("lay");
    let mut store = CharacterStore::default();
    {
        let mut w = world(shipped(), &mut env, &pops, 1);
        store
            .start_768(&mut w, &data_of(&[], &[], LAY_SLOTS), start_day())
            .expect("the start");
    }
    let s = Started {
        store,
        env,
        pops,
        report: StartReport::default(),
    };
    let members = |slot: &str| -> Vec<Character> {
        s.store
            .iter()
            .filter(|c| c.life.slot_key == slot)
            .cloned()
            .collect()
    };
    let living_parents = |c: &Character| -> Vec<Character> {
        [c.father, c.mother]
            .into_iter()
            .filter_map(|id| s.store.get(id))
            .cloned()
            .collect()
    };
    // `hermit` is monastic (a monk pop and no peasant or noble pop): its craftsmen, lay members of the labourer pop, and its monks stay put, their
    // pops hold only them, and their living generated parents live at the nearest Mumu peasant pop, the farm.
    let hermit = members("hermit_slot");
    assert_eq!(hermit.len(), 5);
    assert_eq!(
        s.store.pop_count(&locator("hermit", 1)),
        3,
        "the labourer pop holds only its slot members"
    );
    assert_eq!(
        s.store.pop_count(&locator("hermit", 0)),
        2,
        "the monk pop holds only its slot members"
    );
    let mut away = 0;
    for member in &hermit {
        for parent in living_parents(member) {
            away += 1;
            assert_eq!(parent.pop, locator("farm", 0), "{}", member.name);
        }
    }
    assert!(away > 0, "some parents are alive");
    // `crag` is a noble seat with no peasants: families stay in the character's pop, whatever it is.
    let crag = members("crag_slot");
    assert_eq!(crag.len(), 7);
    let mut stayed = 0;
    for member in &crag {
        for parent in living_parents(member) {
            stayed += 1;
            assert_eq!(
                parent.pop, member.pop,
                "{} at a noble seat keeps its family",
                member.name
            );
        }
    }
    assert!(stayed > 0);
    // At `glen`, which has a peasant pop, a lay member's relatives stay in his pop as well.
    let glen = members("glen_slot");
    assert_eq!(glen.len(), 6);
    for member in &glen {
        assert_eq!(member.pop, locator("glen", 0));
        for parent in living_parents(member) {
            assert_eq!(parent.pop, locator("glen", 0));
        }
    }
    assert_eq!(
        s.store.pop_count(&locator("glen", 1)),
        0,
        "none of them went to the peasants"
    );
}

#[test]
fn a_surviving_parent_of_a_monk_of_a_culture_with_no_other_pop_is_abroad() {
    let s = go("abroad");
    assert!(
        s.report.abroad >= 1,
        "the Northumbrian monks' parents are alive and abroad"
    );
    let monks = ["eng1", "eng2", "eng3", "eng4", "eng5", "eng6"];
    let mut abroad = Vec::new();
    for parent in parents_of(&s, &monks) {
        assert!(
            s.store.get(parent.id).is_none(),
            "no live relative of a Northumbrian monk: {parent:?}"
        );
        if parent.life.departure == Departure::Abroad {
            abroad.push(parent);
        } else {
            assert!(
                parent.death.is_some(),
                "a parent not abroad is dead, with a date"
            );
        }
    }
    assert_eq!(abroad.len(), s.report.abroad);
    for parent in &abroad {
        // Archived with the state, alive or dead unknown to the game: no death, no head, no simulation.
        assert_eq!(parent.death, None);
        assert!(parent.archivable());
        assert!(!parent.children.is_empty());
    }
    // It resolves as a parent through a real reopen of the archive and the canonical encoding.
    let reopened = Archive::open(s.env.archive_path()).expect("reopens");
    let child = named(&s, "eng1");
    let parent_id = [child.father, child.mother]
        .into_iter()
        .find(|id| abroad.iter().any(|p| p.id == *id))
        .or_else(|| abroad.first().map(|p| p.id))
        .expect("a parent abroad");
    let held = reopened.get(parent_id).expect("readable").expect("held");
    assert_eq!(held.life.departure, Departure::Abroad);
    assert_eq!(
        Character::from_bytes(&held.to_bytes().expect("encodes")).expect("decodes"),
        held
    );
    // Abroad parents take no head: the Northumbrian monk pop holds six monks and nobody else.
    assert_eq!(s.store.pop_count(&locator("mayo", 0)), 6);
}

#[test]
fn a_pop_that_would_hold_fewer_heads_than_characters_is_raised_at_the_start_and_each_addition_is_counted()
 {
    // The court's noble pop has room for the figures only: the king's household and the slots overflow it.
    let refused = started("net-off", 1, 12, false);
    assert!(
        matches!(refused, Err(StartError::Engine(_))),
        "without the safety net a full pop refuses a character, as in play"
    );
    let s = started("net-on", 1, 12, true).expect("the start with the safety net");
    let raised: u64 = s.report.raised.iter().map(|(_, n)| n).sum();
    assert!(raised > 0, "the court's pop was raised");
    assert_eq!(
        s.report.raised,
        s.pops.raised_at_start(),
        "the report is the populations' own count"
    );
    for (pop, count) in &s.report.raised {
        assert_eq!(pop.place, "court", "only the crowded pop");
        assert!(*count > 0);
    }
    // Every pop holds at least its characters at the end, and the heads of each pop are what it had, what the net added and what moved.
    let moves = s.pops.moves.borrow();
    for (pop, _, _) in s.pops.all_pops() {
        let characters = s.store.pop_count(&pop) as u64;
        let heads = s.pops.heads(&pop).expect("a pop");
        assert!(
            heads >= characters,
            "{pop:?}: {heads} heads, {characters} characters"
        );
        let initial = pops(12).heads(&pop).expect("a pop");
        let added = s.pops.raised.borrow().get(&pop).copied().unwrap_or(0);
        let moved_in = moves.iter().filter(|(_, to)| *to == pop).count() as u64;
        let moved_out = moves.iter().filter(|(from, _)| *from == pop).count() as u64;
        assert_eq!(heads, initial + added + moved_in - moved_out, "{pop:?}");
    }
    assert!(
        s.report
            .lines()
            .iter()
            .any(|line| line.contains("raised by")),
        "the start log names each raised pop"
    );
    // In play, the refusal stands: a full pop is not raised (the default `make_room` raises nothing).
    let plain = Pops::new().pop("tiny", 0, "noble", "mumu", 1);
    assert!(!plain.make_room(&locator("tiny", 0)).expect("a pop"));
}

#[test]
fn the_burn_in_runs_every_year_once_for_everyone_and_makes_and_removes_no_one() {
    let s = go("burn");
    // Lines dated 753-767 exist: the years of the burn-in, in their own years; and the pasts ended at 752 (nothing is dated twice a year
    // by the yearly stage: the stages of a year run on one birthday of it).
    let mut in_burn_in = 0;
    let mut after_start = 0;
    for c in s.store.iter() {
        for line in s.env.logs.read(c.id).expect("a log") {
            let year = year_of(line.date);
            if (753..=767).contains(&year) {
                in_burn_in += 1;
            }
            if line.date > start_day() {
                after_start += 1;
            }
        }
    }
    assert!(in_burn_in > 0, "events accumulate over the burn-in");
    assert_eq!(after_start, 0, "nothing is dated after the start");
    // A second run of the burn-in over the same characters changes who is alive and who is archived in no way: no deaths and no births.
    let mut s = s;
    let before: Vec<u64> = s.store.iter().map(|c| c.id).collect();
    let archived_before = s.env.archived_ids();
    {
        let mut w = world(shipped(), &mut s.env, &s.pops, 1);
        s.store
            .run_start_burn_in(&mut w, start_day())
            .expect("the burn-in");
    }
    assert_eq!(
        s.store.iter().map(|c| c.id).collect::<Vec<_>>(),
        before,
        "no one was born or died"
    );
    assert_eq!(s.env.archived_ids(), archived_before, "no one was archived");
    // Strands accumulate between characters who live near each other.
    let linked = s.store.iter().filter(|c| !c.ties.links.is_empty()).count();
    assert!(linked > 0, "strands were made");
}

#[test]
fn a_generated_past_stops_at_the_end_of_752_and_the_burn_in_runs_the_rest() {
    let pops = pops(400);
    let mut env = Env::new("past");
    let mut store = CharacterStore::default();
    store.begin_start(date(753, 1, 1));
    let id = {
        let mut w = world(shipped(), &mut env, &pops, 3);
        let mut request = ck_characters::generator::GenerateRequest::new(
            locator("court", 0),
            Sex::Male,
            ck_characters::generator::Birth::On(date(700, 3, 3)),
            "old",
            start_day(),
        );
        request.house = "ui_x".to_owned();
        store.generate(&mut w, &request).expect("a generation").id
    };
    store.end_start();
    let last = env
        .logs
        .read(id)
        .expect("a log")
        .iter()
        .map(|line| line.date)
        .max();
    assert!(
        last.is_some_and(|day| day < date(753, 1, 1)),
        "{last:?}: the generated past ended with 752"
    );
    // A character born after 752 has no past at all.
    let mut w = world(shipped(), &mut env, &pops, 3);
    store.begin_start(date(753, 1, 1));
    let young = ck_characters::generator::GenerateRequest::new(
        locator("court", 0),
        Sex::Female,
        ck_characters::generator::Birth::On(date(760, 1, 1)),
        "young",
        start_day(),
    );
    let young_id = store.generate(&mut w, &young).expect("a generation").id;
    store.end_start();
    assert!(env.logs.read(young_id).expect("a log").is_empty());
    // The burn-in then runs 753-767 for the first and skips the second until its first birthday.
    {
        let mut w = world(shipped(), &mut env, &pops, 3);
        store
            .run_start_burn_in(&mut w, start_day())
            .expect("the burn-in");
    }
    let lines = env.logs.read(id).expect("a log");
    assert!(
        lines
            .iter()
            .any(|line| (753..=767).contains(&year_of(line.date)))
    );
    assert!(lines.iter().all(|line| line.date < start_day()));
    assert!(
        env.logs
            .read(young_id)
            .expect("a log")
            .iter()
            .all(|line| year_of(line.date) >= 761)
    );
}

#[test]
fn the_known_strands_hold_between_live_characters_and_a_dead_ones_is_in_its_archived_record() {
    let s = go("strands");
    let (abbot, isle1) = (live(&s, "Zzabbot"), live(&s, "isle1"));
    // Between two live characters: the authored strength, on both members (a rivalry is a bond), whatever the burn-in made.
    assert_eq!(abbot.ties.strength(isle1.id, "rival"), 2);
    assert_eq!(isle1.ties.strength(abbot.id, "rival"), 2);
    // With a dead one: written into its archived record when it was appended (read back through a real reopen), with no second record, and the
    // living partner holds nothing of it (a history, not a bond).
    let reopened = Archive::open(s.env.archive_path()).expect("reopens");
    let may_id = named(&s, "Zzmay").id;
    let may = reopened.get(may_id).expect("readable").expect("held");
    assert_eq!(may.ties.strength(abbot.id, "rival"), 3);
    assert_eq!(abbot.ties.strength(may_id, "rival"), 0);
    assert_eq!(reopened.ids().filter(|id| *id == may_id).count(), 1);
    assert_eq!(
        s.report.strands, 1,
        "only the strand between two live characters was set after the burn-in"
    );
}

#[test]
fn two_starts_on_one_seed_are_the_same_and_another_seed_differs() {
    let (a, b, c) = (
        started("det-a", 11, 400, false).expect("a start"),
        started("det-b", 11, 400, false).expect("a start"),
        started("det-c", 12, 400, false).expect("a start"),
    );
    let digest = |s: &Started| {
        let mut bytes = s.store.to_bytes().expect("encodes");
        for id in s.env.archived_ids() {
            bytes.extend(
                s.env
                    .archive
                    .get(id)
                    .expect("readable")
                    .expect("held")
                    .to_bytes()
                    .expect("encodes"),
            );
        }
        for c in s.store.iter() {
            for line in s.env.logs.read(c.id).expect("a log") {
                bytes.extend(format!("{}:{:?}:{}", line.id, line.date, line.text).bytes());
            }
        }
        ck_sim::fnv1a(&bytes)
    };
    assert_eq!(
        digest(&a),
        digest(&b),
        "the same seed makes the same state, archive and dated logs"
    );
    assert_ne!(digest(&a), digest(&c), "another seed differs");
    assert_eq!(a.report, b.report);
}

#[test]
fn the_start_is_the_one_thing_that_places_relatives_beside_monks() {
    // Outside the start, a generated parent of a monk is placed in the monk's own pop, as slice 7 made it.
    let pops = pops(400);
    let mut env = Env::new("play");
    let mut store = CharacterStore::default();
    let id = {
        let mut w = world(shipped(), &mut env, &pops, 5);
        let request = ck_characters::generator::GenerateRequest::new(
            locator("isle", 0),
            Sex::Male,
            ck_characters::generator::Birth::On(date(752, 1, 1)),
            "monk",
            start_day(),
        );
        store.generate(&mut w, &request).expect("a generation").id
    };
    let monk = store.get(id).expect("live").clone();
    for parent in [monk.father, monk.mother] {
        if let Some(live) = store.get(parent) {
            assert_eq!(
                live.pop,
                locator("isle", 0),
                "in play the relatives are in the pop"
            );
        }
    }
}

/// A hundred men of the court (born 738 to 757) with no wife named: their households generate wives and children (CK-370 §5), on a pop with room for all of them.
fn sample() -> FigureData {
    let figures: Vec<Fig> = (0..100_u16)
        .map(|n| {
            let key: &'static str = Box::leak(format!("sample{n}").into_boxed_str());
            Fig {
                name: key,
                house: "ui_s",
                ..fig(key, "court", "noble", "m", 738 + n % 20)
            }
        })
        .collect();
    let (research, load) = files(&figures, &[], &[], "", EXCLUDED);
    FigureData::parse(&research, &load).expect("the sample loads")
}

fn sample_start(tag: &str, seed: u64, lifecycle: &'static ck_economy::LifeCycle) -> Started {
    let pops = pops(100_000);
    let mut env = Env::new(tag);
    let mut store = CharacterStore::default();
    let report = {
        let mut w = world(lifecycle, &mut env, &pops, seed);
        store
            .start_768(&mut w, &sample(), start_day())
            .expect("the sample start")
    };
    Started {
        store,
        env,
        pops,
        report,
    }
}

/// The records of the generated children of the sample's men, live and archived, and the men.
fn sample_children(s: &Started) -> (Vec<Character>, Vec<Character>) {
    let men: Vec<Character> = s
        .store
        .iter()
        .filter(|c| c.name.starts_with("sample"))
        .cloned()
        .collect();
    let ids: Vec<u64> = men.iter().map(|c| c.id).collect();
    let live = s.store.iter().filter(|c| ids.contains(&c.father)).cloned();
    let dead = s
        .env
        .archived_ids()
        .into_iter()
        .filter_map(|id| s.env.archive.get(id).expect("readable"))
        .filter(|c| ids.contains(&c.father));
    (men, live.chain(dead).collect())
}

#[test]
fn with_the_death_curve_on_generated_children_die_and_are_archived_dated_and_linked() {
    let s = sample_start("curve-on", 3, shipped());
    let (men, children) = sample_children(&s);
    assert_eq!(
        men.len(),
        100,
        "every figure is live: only the relatives die"
    );
    assert!(children.len() > 150, "a large sample: {}", children.len());
    let dead: Vec<&Character> = children.iter().filter(|c| c.death.is_some()).collect();
    let (dead_count, total) = (dead.len(), children.len());
    assert!(
        (20 * total..=45 * total).contains(&(100 * dead_count)),
        "{dead_count} of {total}: the share is outside 0.20 to 0.45"
    );
    for child in dead {
        let died = child.death.expect("a date");
        assert!(died >= child.birth && died <= start_day(), "{child:?}");
        assert!(child.archivable());
        assert!(s.store.get(child.id).is_none(), "archived, not live");
        let father = s.store.get(child.father).expect("a live father");
        assert!(father.children.contains(&child.id), "linked to its family");
        assert_eq!(
            support::record(&s.store, &s.env, child.id).death,
            child.death
        );
    }
}

#[test]
fn a_generated_wife_can_die_at_a_birth_and_is_archived_on_that_day() {
    let mut found = Vec::new();
    let mut wives = 0;
    for seed in 1..=4 {
        let s = sample_start(&format!("wives-{seed}"), seed, shipped());
        let (men, children) = sample_children(&s);
        let mut mothers: Vec<u64> = children.iter().map(|c| c.mother).collect();
        mothers.sort_unstable();
        mothers.dedup();
        for mother in mothers {
            wives += 1;
            let wife = support::record(&s.store, &s.env, mother);
            if wife.life.death_cause.as_deref() == Some("childbirth") {
                // Archived on the day of a delivery, with no child of hers born after it.
                let died = wife.death.expect("a date");
                let mine: Vec<&Character> =
                    children.iter().filter(|c| c.mother == mother).collect();
                assert!(
                    mine.iter().any(|c| c.birth == died),
                    "dated at a birth: {died:?}"
                );
                assert!(mine.iter().all(|c| c.birth <= died));
                assert!(s.store.get(mother).is_none());
                assert!(
                    men.iter().any(|m| m.spouses.contains(&mother)),
                    "still her husband's spouse"
                );
                found.push(mother);
            }
        }
    }
    assert!(wives > 100, "{wives}");
    assert!(
        !found.is_empty(),
        "a childbirth death is possible: none among {wives} wives"
    );
}

#[test]
fn with_the_death_curve_off_every_generated_relative_survives_as_in_the_first_draft() {
    let s = sample_start("curve-off", 3, without_relative_deaths());
    let (men, children) = sample_children(&s);
    assert_eq!(men.len(), 100);
    assert!(children.len() > 150);
    assert!(
        children
            .iter()
            .all(|c| c.death.is_none() && s.store.get(c.id).is_some()),
        "every child is live"
    );
    // Every wife is live too: no childbirth death.
    for child in &children {
        assert!(s.store.get(child.mother).is_some());
    }
    assert!(s.env.archived_ids().iter().all(|id| {
        let c = s.env.archive.get(*id).expect("readable").expect("held");
        c.life.death_cause.as_deref() != Some("childbirth")
    }));
}

#[test]
fn the_ages_of_generated_slot_members_come_from_the_data_and_a_role_not_in_it_takes_other() {
    // The shipped table: a retainer is 16 to 45, a court cleric 25 to 60.
    let s = go("ages");
    let age = |c: &Character| {
        let (born, now) = (
            c.birth.date().expect("a date"),
            start_day().date().expect("a date"),
        );
        u32::from(now.year - born.year) - u32::from((now.month, now.day) < (born.month, born.day))
    };
    let retainers: Vec<_> = s
        .store
        .iter()
        .filter(|c| c.life.role == "retainer")
        .collect();
    assert_eq!(retainers.len(), 3);
    assert!(
        retainers.iter().all(|c| (16..=45).contains(&age(c))),
        "{:?}",
        retainers.iter().map(|c| age(c)).collect::<Vec<_>>()
    );
    let cleric = s
        .store
        .iter()
        .find(|c| c.life.role == "court_cleric")
        .expect("a cleric");
    assert!((25..=60).contains(&age(cleric)));
    // A table of the test's own: retainers 20 to 21, `other` 40 to 41 for every role it does not name.
    let mut custom = shipped().clone();
    custom.generator.slots.member_ages = [
        ("retainer".to_owned(), [20, 21]),
        ("other".to_owned(), [40, 41]),
    ]
    .into();
    let custom: &'static ck_economy::LifeCycle = Box::leak(Box::new(custom));
    let pops = pops(400);
    let mut env = Env::new("ages-custom");
    let mut store = CharacterStore::default();
    {
        let mut w = world(custom, &mut env, &pops, 1);
        store
            .start_768(&mut w, &data(), start_day())
            .expect("a start");
    }
    let by_role = |role: &str| -> Vec<u32> {
        store
            .iter()
            .filter(|c| c.life.role == role)
            .map(age)
            .collect()
    };
    assert!(
        by_role("retainer").iter().all(|a| (20..=21).contains(a)),
        "{:?}",
        by_role("retainer")
    );
    assert!(
        by_role("household_woman")
            .iter()
            .all(|a| (40..=41).contains(a)),
        "not in the table: other"
    );
    assert!(by_role("scribe").iter().all(|a| (40..=41).contains(a)));
    assert!(!by_role("scribe").is_empty());
}

#[test]
fn a_slot_row_places_only_its_authored_sex_and_a_craft_role_has_the_craft_at_c() {
    for seed in 1..=4 {
        let s = started("slot-sex", seed, 400, false).expect("the start");
        // Fosterlings of a male row are boys and of a female row girls, whether chosen from the existing children or generated for the place.
        let of = |slot: &str| -> Vec<Character> {
            s.store
                .iter()
                .filter(|c| c.life.slot_key == slot)
                .cloned()
                .collect()
        };
        let (boys, girls) = (of("fost_m"), of("fost_f"));
        assert_eq!(boys.len(), 3, "seed {seed}");
        assert_eq!(girls.len(), 3, "seed {seed}");
        assert!(
            boys.iter()
                .all(|c| c.sex == Sex::Male && c.life.slot_kind == "fosterling"),
            "seed {seed}"
        );
        assert!(
            girls
                .iter()
                .all(|c| c.sex == Sex::Female && c.life.slot_kind == "fosterling"),
            "seed {seed}"
        );
        // The students of the church's male row are boys.
        assert!(
            of("church_slot")
                .iter()
                .filter(|c| c.life.slot_kind == "student")
                .all(|c| c.sex == Sex::Male),
            "seed {seed}"
        );
    }
    let s = go("slot-craft");
    // A role that is a craft (`judge`, `poet`, `scribe`, `herdsman`) grants it at C (grade 1) from the start.
    for role in ["judge", "poet", "herdsman", "scribe"] {
        let members: Vec<_> = s.store.iter().filter(|c| c.life.role == role).collect();
        assert!(!members.is_empty(), "{role}");
        assert!(
            members.iter().all(|c| c.grade(role) >= 1),
            "{role} has its craft"
        );
    }
    // A role that is not a craft grants nothing: a retainer has no craft of that name.
    assert!(
        s.store
            .iter()
            .filter(|c| c.life.role == "retainer")
            .all(|c| c.grade("retainer") == 0)
    );
}

#[test]
fn the_slot_a_member_or_a_spouse_fills_is_recorded_and_a_spouse_already_placed_fills_no_second_place()
 {
    let s = go("slot-record");
    // Ordinary members carry the slot's key and their role as the kind of their place.
    for retainer in s.store.iter().filter(|c| c.life.role == "retainer") {
        assert!(
            ["court_slot", "lord_slot"].contains(&retainer.life.slot_key.as_str()),
            "{:?}",
            retainer.life.slot_key
        );
        assert_eq!(retainer.life.slot_kind, "retainer");
    }
    let lord = s
        .store
        .iter()
        .find(|c| c.life.role == "lord")
        .expect("a lord");
    assert_eq!(
        (lord.life.slot_key.as_str(), lord.life.slot_kind.as_str()),
        ("lord_slot", "lord")
    );
    // The named queen fills `court_slot`'s spouse place, with its role as the kind.
    let queen = live(&s, "Zzqueen");
    assert_eq!(
        (queen.life.slot_key.as_str(), queen.life.slot_kind.as_str()),
        ("court_slot", "spouse_of_ruler")
    );
    // The generated wife of the lord, and a spouse place of the same ruler in another slot: she already fills one, so it stays unfilled.
    let wife = lord
        .spouses
        .iter()
        .filter_map(|id| s.store.get(*id))
        .find(|c| c.life.role == "spouse_of_lord")
        .expect("the lord's wife");
    assert_eq!(
        (wife.life.slot_key.as_str(), wife.life.slot_kind.as_str()),
        ("lord_slot", "spouse_of_lord")
    );
    assert!(
        s.report
            .warnings
            .iter()
            .any(|w| w.contains("spouse_again")
                && w.contains("already fills a place of `court_slot`")),
        "{:?}",
        s.report.warnings
    );
    assert_eq!(
        queen.life.slot_key, "court_slot",
        "she stays where she was placed"
    );
}

/// The shipped life-cycle data edited by `edit`, leaked for the life of the test.
fn custom(edit: impl FnOnce(&mut ck_economy::LifeCycle)) -> &'static ck_economy::LifeCycle {
    let mut data = shipped().clone();
    edit(&mut data);
    Box::leak(Box::new(data))
}

/// A data set of `figures` alone (no dead parents, strands or slots beyond `slots`).
fn data_of(figures: &[Fig], strands: &[(&str, &str, &str, u8)], slots: &str) -> FigureData {
    let (research, load) = files(figures, &[], strands, slots, EXCLUDED);
    FigureData::parse(&research, &load).expect("the data loads")
}

/// Every archived record of a start.
fn archived(s: &Started) -> Vec<Character> {
    s.env
        .archived_ids()
        .into_iter()
        .filter_map(|id| s.env.archive.get(id).expect("readable"))
        .collect()
}

#[test]
fn a_known_year_is_kept_a_later_death_is_history_and_creation_follows_parents_then_birth_then_key()
{
    let figures = vec![
        Fig {
            name: "Zzlate",
            alive: false,
            before_start: true,
            died: Some(762),
            ..fig("late", "court", "noble", "m", 720)
        },
        Fig {
            name: "Zzfuture",
            died: Some(780),
            died_date: "0780-01-05",
            ..fig("future", "court", "noble", "m", 730)
        },
        Fig {
            name: "Zztiea",
            ..fig("tie_a", "court", "noble", "m", 740)
        },
        Fig {
            name: "Zztieb",
            ..fig("tie_b", "court", "noble", "m", 740)
        },
        Fig {
            name: "Zzkid",
            father: "tie_b",
            ..fig("kid", "court", "noble", "m", 741)
        },
    ];
    let s = started_on("order", 1, 400, false, &data_of(&figures, &[], "")).expect("the start");
    // `died_before_start` with a known year keeps the year (1 January), and the unknown value is for records with no date at all.
    let late = named(&s, "Zzlate");
    assert_eq!(late.death, Some(date(762, 1, 1)));
    assert_eq!(late.life.departure, Departure::None);
    // A living figure's later death is history: it is live.
    assert!(s.store.get(live(&s, "Zzfuture").id).is_some());
    // Ties by birth date then key; a child after its parent.
    let (a, b, kid) = (live(&s, "Zztiea"), live(&s, "Zztieb"), live(&s, "Zzkid"));
    assert!(a.id < b.id, "equal births are made in key order");
    assert_eq!(kid.father, b.id);
    assert!(b.id < kid.id);
}

#[test]
fn a_historical_link_to_a_character_made_later_is_written_when_the_record_is_appended() {
    let figures = vec![
        Fig {
            name: "Zzgrandsire",
            house: "ui_a",
            alive: false,
            died: Some(750),
            ..fig("grandsire", "court", "noble", "m", 700)
        },
        Fig {
            name: "Zzheir",
            house: "ui_a",
            father: "grandsire",
            ..fig("heir", "court", "noble", "m", 725)
        },
        Fig {
            name: "Zzdeada",
            alive: false,
            died: Some(745),
            ..fig("deada", "court", "noble", "f", 720)
        },
        Fig {
            name: "Zzdeadb",
            alive: false,
            died: Some(748),
            ..fig("deadb", "court", "noble", "m", 722)
        },
        Fig {
            name: "Zzmum",
            alive: false,
            died: Some(750),
            spouse: "dad2",
            ..fig("mum", "court", "noble", "f", 700)
        },
        Fig {
            name: "Zzdad2",
            spouse: "mum",
            ..fig("dad2", "court", "noble", "m", 710)
        },
        Fig {
            name: "Zzson",
            father: "dad2",
            mother: "mum",
            ..fig("son", "court", "noble", "m", 730)
        },
    ];
    let strands = [
        ("grandsire", "heir", "rival", 2),
        ("deada", "deadb", "friend", 1),
        ("heir", "deada", "love", 3),
        ("deadb", "heir", "respect", 2),
    ];
    let s =
        started_on("links", 1, 400, false, &data_of(&figures, &strands, "")).expect("the start");
    let reopened = Archive::open(s.env.archive_path()).expect("reopens");
    let held = |name: &str| {
        let id = named(&s, name).id;
        reopened.get(id).expect("readable").expect("held")
    };
    let (grandsire, first_dead, second_dead, mum) = (
        held("Zzgrandsire"),
        held("Zzdeada"),
        held("Zzdeadb"),
        held("Zzmum"),
    );
    let (heir, dad2) = (live(&s, "Zzheir"), live(&s, "Zzdad2"));
    // A bond between an archived ancestor and its later-made descendant is on the archived end only, and is history: the living end holds none.
    assert_eq!(grandsire.ties.strength(heir.id, "rival"), 2);
    assert_eq!(heir.ties.strength(grandsire.id, "rival"), 0);
    // A bond joining two archived figures is on both records.
    assert_eq!(first_dead.ties.strength(second_dead.id, "friend"), 1);
    assert_eq!(second_dead.ties.strength(first_dead.id, "friend"), 1);
    // A one-way feeling is on the archived record only when the archived figure is the one that holds it (`a` toward `b`).
    assert_eq!(
        second_dead.ties.strength(heir.id, "respect"),
        2,
        "deadb holds respect for the heir"
    );
    // A one-way feeling from a living figure toward an archived one is kept as it was authored: inbound history on the archived record (through
    // the reopen), never an outgoing feeling of the archived figure and never a live bond.
    assert_eq!(
        first_dead.ties.inbound_strength(heir.id, "love"),
        3,
        "the heir's authored love for deada"
    );
    assert_eq!(
        first_dead.ties.strength(heir.id, "love"),
        0,
        "not deada's own feeling"
    );
    assert_eq!(
        heir.ties.strength(first_dead.id, "love"),
        0,
        "and history, not a live bond"
    );
    // An archived spouse made before the living one it is married to lists it, and the living one lists her.
    assert!(mum.id < dad2.id);
    assert!(mum.spouses.contains(&dad2.id));
    assert!(dad2.spouses.contains(&mum.id));
    assert_eq!(live(&s, "Zzson").mother, mum.id);
    // No second record: each archived figure is held once.
    for name in ["Zzgrandsire", "Zzdeada", "Zzdeadb", "Zzmum"] {
        let id = named(&s, name).id;
        assert_eq!(reopened.ids().filter(|held| *held == id).count(), 1);
    }
}

#[test]
fn an_abroad_parent_never_rolls_a_death_and_has_no_pop_while_an_on_map_one_may_die() {
    let s = go("abroad-roll");
    let mut dead_on_map = 0;
    for name in [
        "isle1", "isle2", "isle3", "isle4", "isle5", "isle6", "Zzabbot",
    ] {
        let child = named(&s, name);
        for parent in [child.father, child.mother] {
            if support::record(&s.store, &s.env, parent).death.is_some() {
                dead_on_map += 1;
            }
        }
    }
    assert!(
        dead_on_map > 0,
        "the on-map control relatives die by the curve, with dates"
    );
    let reopened = Archive::open(s.env.archive_path()).expect("reopens");
    let mut abroad = 0;
    for name in ["eng1", "eng2", "eng3", "eng4", "eng5", "eng6"] {
        let child = named(&s, name);
        for parent in [child.father, child.mother] {
            let record = reopened.get(parent).expect("readable").expect("held");
            assert_eq!(
                record.life.departure,
                Departure::Abroad,
                "{name}'s parent is abroad: it never reached the curve"
            );
            assert_eq!(record.death, None, "no date");
            assert!(
                record.pop.place.is_empty(),
                "no on-map pop: {:?}",
                record.pop
            );
            assert!(record.archivable());
            // The family lookup resolves through it, and the canonical encoding keeps it.
            assert!(record.children.contains(&child.id));
            assert_eq!(support::record(&s.store, &s.env, parent), record);
            assert_eq!(
                Character::from_bytes(&record.to_bytes().expect("encodes")).expect("decodes"),
                record
            );
            abroad += 1;
        }
    }
    assert_eq!(abroad, 12);
    assert_eq!(s.report.abroad, 12);
}

/// A ruler at `fort` and the slot that names his wife.
const FORT_SLOT: &str = r#"
[[slot]]
key = "fort_slot"
location = "fort"
[[slot.member]]
role = "spouse_of_ruler"
count = 1
pop = "noble"
sex = "f"
"#;

fn age_at_start(c: &Character) -> u32 {
    let (born, now) = (
        c.birth.date().expect("a date"),
        start_day().date().expect("a date"),
    );
    u32::from(now.year - born.year) - u32::from((now.month, now.day) < (born.month, born.day))
}

/// No man of the data marries of his own accord: the household draws no wife, and a spouse place is the only way he gets one.
fn unwed(edit: impl FnOnce(&mut ck_economy::LifeCycle)) -> &'static ck_economy::LifeCycle {
    custom(|l| {
        l.marriage.wish.men = vec![(14, 0), (90, 0)];
        l.marriage.wish.women = vec![(12, 0), (60, 0)];
        edit(l);
    })
}

#[test]
fn a_spouse_place_always_gets_a_living_marriageable_wife_in_the_stated_window() {
    // A ruler of 78: no fertile age lies within his free gap, so the fertility limit is dropped and her age is in `[max(M, A - G), A + G]`.
    let old = vec![Fig {
        name: "Zzold",
        house: "ui_o",
        office: "ruler",
        ..fig("old", "fort", "noble", "m", 690)
    }];
    let lifecycle = unwed(|_| {});
    let (gap, minimum) = (
        lifecycle.marriage.score.age_gap_free,
        lifecycle.marriageable["christianity"].f,
    );
    for seed in 1..=6 {
        let s = started_with(
            "tier1",
            seed,
            400,
            false,
            &data_of(&old, &[], FORT_SLOT),
            (lifecycle, &support::rules().events),
        )
        .expect("the start");
        let ruler = live(&s, "Zzold");
        let wife = ruler
            .spouses
            .iter()
            .filter_map(|id| s.store.get(*id))
            .find(|c| c.life.slot_key == "fort_slot")
            .expect("a living wife");
        assert_eq!(
            (wife.life.slot_kind.as_str(), wife.sex),
            ("spouse_of_ruler", Sex::Female)
        );
        let age = age_at_start(wife);
        assert!(
            age >= minimum && (age >= 78 - gap) && age <= 78 + gap,
            "seed {seed}: age {age}, gap {gap}, minimum {minimum}"
        );
        assert!(wife.spouses.contains(&ruler.id));
    }
    // A ruler of 14, a free gap of 1 and a marriageable minimum of 16: the window `[max(16, 13), 15]` is empty, so she is the minimum, 16.
    let boy = vec![Fig {
        name: "Zzboy",
        house: "ui_o",
        office: "ruler",
        ..fig("boy", "fort", "noble", "m", 754)
    }];
    let lifecycle = unwed(|l| {
        l.marriage.score.age_gap_free = 1;
        l.marriageable
            .get_mut("christianity")
            .expect("christianity")
            .f = 16;
    });
    for seed in 1..=4 {
        let s = started_with(
            "tier2",
            seed,
            400,
            false,
            &data_of(&boy, &[], FORT_SLOT),
            (lifecycle, &support::rules().events),
        )
        .expect("the start");
        let ruler = live(&s, "Zzboy");
        assert_eq!(age_at_start(&ruler), 14);
        let wife = ruler
            .spouses
            .iter()
            .filter_map(|id| s.store.get(*id))
            .find(|c| c.life.slot_key == "fort_slot")
            .expect("a living wife");
        assert_eq!(
            age_at_start(wife),
            16,
            "seed {seed}: the youngest marriageable candidate"
        );
    }
}

/// Life-cycle data in which every man and woman marries in the first year of marriageable age and every month is a conception, so that a rule
/// of the household's timeline decides what happens and no chance does; `edit` forces the deaths.
fn certain(edit: impl FnOnce(&mut ck_economy::LifeCycle)) -> &'static ck_economy::LifeCycle {
    let full = u64::from(ck_economy::genetics::PPB);
    custom(|l| {
        l.marriage.wish.men = vec![(14, full), (90, full)];
        l.marriage.wish.women = vec![(12, full), (60, full)];
        l.conception.monthly_ppb = u32::try_from(full).expect("a probability");
        l.conception.women = vec![(15, full), (60, full)];
        l.conception.men = vec![(15, full), (90, full)];
        l.conception.church_abstinence_ppb = full;
        l.conception.twins_ppb = 0;
        l.conception.triplets_ppb = 0;
        edit(l);
    })
}

/// Everyone made at the start, live and archived.
fn everyone(s: &Started) -> Vec<Character> {
    s.store.iter().cloned().chain(archived(s)).collect()
}

fn men_of(prefix: &str, place: &'static str, sex: &'static str, count: u16, from: u16) -> Vec<Fig> {
    (0..count)
        .map(|n| {
            let key: &'static str = Box::leak(format!("{prefix}{n}").into_boxed_str());
            Fig {
                name: key,
                house: "ui_t",
                ..fig(key, place, "noble", sex, from + n % 18)
            }
        })
        .collect()
}

#[test]
fn a_spouse_a_slot_names_does_not_roll_and_an_unslotted_generated_wife_dies_at_her_first_delivery_when_forced()
 {
    // Every delivery kills the wife: the ruler's wife (named by the slot, made by his household) must live, and an unslotted man's wife dies at
    // her first delivery, on its day, with no later birth.
    let lifecycle = certain(|l| {
        l.death.childbirth_ppb = ck_economy::genetics::PPB;
    });
    let mut figures = men_of("plain", "fort", "m", 20, 738);
    figures.push(Fig {
        name: "Zzking2",
        house: "ui_w",
        office: "ruler",
        ..fig("king2", "fort", "noble", "m", 738)
    });
    let s = started_with(
        "protected",
        1,
        100_000,
        true,
        &data_of(&figures, &[], FORT_SLOT),
        (lifecycle, &support::rules().events),
    )
    .expect("the start");
    let king2 = live(&s, "Zzking2");
    let wife = king2
        .spouses
        .iter()
        .find_map(|id| s.store.get(*id))
        .expect("the ruler's wife is alive");
    assert!(wife.life.death_cause.is_none() && s.store.get(wife.id).is_some());
    assert_eq!(
        wife.life.slot_key, "fort_slot",
        "she is the one the slot names"
    );
    let kids = everyone(&s)
        .into_iter()
        .filter(|c| c.mother == wife.id)
        .count();
    assert!(kids >= 2, "she bore child after child: {kids}");
    // The control wives: each who gave birth died at that delivery, and none had a later one.
    let people = everyone(&s);
    let mut died = 0;
    for n in 0..20 {
        let man = live(&s, &format!("plain{n}"));
        for spouse in &man.spouses {
            let wife = support::record(&s.store, &s.env, *spouse);
            let births: Vec<GameTime> = people
                .iter()
                .filter(|c| c.mother == wife.id)
                .map(|c| c.birth)
                .collect();
            let Some(first) = births.iter().min() else {
                continue;
            };
            assert_eq!(
                wife.life.death_cause.as_deref(),
                Some("childbirth"),
                "plain{n}'s wife"
            );
            assert_eq!(wife.death, Some(*first), "dated on the day of the delivery");
            assert!(
                births.iter().all(|b| b == first),
                "no delivery after a fatal one"
            );
            assert!(s.store.get(wife.id).is_none());
            died += 1;
        }
    }
    assert!(died >= 10, "{died}");
}

#[test]
fn a_father_who_dies_after_conceiving_still_has_the_child_and_a_mother_who_dies_pregnant_loses_it()
{
    // Women of the court with no husband named: their husbands are generated and die on their 26th birthday (a forced draw).
    let lifecycle = certain(|l| l.death.certain_at = 26);
    let s = started_with(
        "father",
        1,
        100_000,
        false,
        &data_of(&men_of("fem", "court", "f", 60, 738), &[], ""),
        (lifecycle, &support::rules().events),
    )
    .expect("the start");
    let people = everyone(&s);
    let (mut posthumous, mut checked) = (0, 0);
    let husbands: Vec<Character> = (0..60)
        .flat_map(|n| live(&s, &format!("fem{n}")).spouses)
        .map(|id| support::record(&s.store, &s.env, id))
        .filter(|c| c.death.is_some())
        .collect();
    assert!(!husbands.is_empty(), "the forced draw killed some husbands");
    for husband in husbands {
        let died = husband.death.expect("a date");
        for child in people.iter().filter(|c| c.father == husband.id) {
            checked += 1;
            // Conceived while he lived (the 1st of a month before his death), delivered perhaps after it.
            assert!(
                months_before_of(child.birth, 9) < died,
                "conceived after his death: {:?} {died:?}",
                child.birth
            );
            posthumous += usize::from(child.birth > died);
        }
    }
    assert!(
        checked > 0 && posthumous > 0,
        "a child delivered after its father died: {posthumous} of {checked}"
    );
    // Wives made for men of the court die on their 30th birthday: no child of theirs is born after it.
    let lifecycle = certain(|l| l.death.certain_at = 30);
    let s = started_with(
        "mother",
        1,
        100_000,
        false,
        &data_of(&men_of("man", "court", "m", 60, 738), &[], ""),
        (lifecycle, &support::rules().events),
    )
    .expect("the start");
    let people = everyone(&s);
    let mut wives = 0;
    for wife in archived(&s).into_iter().filter(|c| {
        c.sex == Sex::Female
            && c.death.is_some()
            && c.life.death_cause.as_deref() != Some("childbirth")
    }) {
        wives += 1;
        let died = wife.death.expect("a date");
        assert!(
            people
                .iter()
                .filter(|c| c.mother == wife.id)
                .all(|c| c.birth <= died),
            "a child born to a mother after her death"
        );
    }
    assert!(
        wives > 0,
        "some wives died of natural causes before the start"
    );
    // A pregnancy carried into the opening always has a living mother: no archived record is pregnant.
    assert!(
        archived(&s)
            .iter()
            .all(|c| c.life.pregnant_by.is_none() && c.life.due.is_none())
    );
    for c in s.store.iter().filter(|c| c.life.pregnant_by.is_some()) {
        assert!(c.life.due.is_some_and(|due| due > start_day()) && c.sex == Sex::Female);
    }
}

/// The date `months` months before `time`, on its day (for the conception of a child).
fn months_before_of(time: GameTime, months: u32) -> GameTime {
    support::months_before(time, months)
}

#[test]
fn a_generated_spouses_death_rolls_start_on_the_first_of_january_after_the_wedding() {
    // A forced death on the 20th birthday: a wife wed at 14 or 15 would die on it, and only if it falls after the 1 January that follows the
    // wedding. One wed in the year before it is never killed on a birthday that falls before that January.
    let lifecycle = certain(|l| {
        l.death.certain_at = 20;
        for ages in l.marriageable.values_mut() {
            ages.m = 14;
            ages.f = 14;
        }
    });
    let pops = pops(100_000);
    let mut env = Env::new("wedding-year");
    let mut store = CharacterStore::default();
    store.begin_start(start_day());
    let (mut weddings, mut dead, mut forced) = (0, 0, 0);
    for n in 0..60_u16 {
        let mut w = world(lifecycle, &mut env, &pops, u64::from(n) + 1);
        let mut request = ck_characters::generator::GenerateRequest::new(
            locator("court", 0),
            Sex::Male,
            ck_characters::generator::Birth::On(date(
                738 + n % 18,
                1 + u8::try_from(n % 12).expect("a month"),
                3,
            )),
            &format!("husband{n}"),
            start_day(),
        );
        request.house = "ui_x".to_owned();
        let id = store.generate(&mut w, &request).expect("a man").id;
        let out = store
            .complete_household(&mut w, id, start_day())
            .expect("a household");
        let (Some(spouse), Some(wedding)) =
            (out.spouse.filter(|_| out.spouse_generated), out.wedding)
        else {
            continue;
        };
        weddings += 1;
        let record = support::record_in(&store, &env.archive, spouse);
        if let Some(died) = record.death {
            dead += 1;
            // A natural death falls on a 1 January: the household's round of each year is taken on that day. The forced draw (the 16th
            // birthday) is the 1 January of the year of that birthday, exactly.
            if record.life.death_cause.as_deref() != Some("childbirth") {
                assert_eq!(
                    died,
                    date(year_of(died), 1, 1),
                    "natural deaths are dated 1 January"
                );
            }
            if record.life.death_cause.as_deref() == Some(ck_characters::cycle::CAUSE_OLD_AGE) {
                // The round is taken on its own date: the certain death is the first 1 January on or after the 20th birthday, never the January
                // of the year before it.
                let born = record.birth.date().expect("a date");
                let birthday = date(born.year + 20, born.month, born.day);
                let first_january = if birthday == date(born.year + 20, 1, 1) {
                    birthday
                } else {
                    date(born.year + 21, 1, 1)
                };
                // Her rolls start on the January after the wedding, so one who was already 20 then dies at that first round.
                let first_round = date(year_of(wedding) + 1, 1, 1);
                assert_eq!(
                    died,
                    first_january.max(first_round),
                    "certain at the first round that finds her 20"
                );
                forced += 1;
            }
            let next_january = date(year_of(wedding) + 1, 1, 1);
            assert!(
                died >= next_january,
                "wed {wedding:?}, dead {died:?}: a roll before the January after the wedding"
            );
        }
    }
    assert!(weddings > 20, "{weddings}");
    assert!(
        dead > 0,
        "the forced draw killed some wives, after that January"
    );
    assert!(
        forced > 0,
        "some wives died of the forced draw, on its January"
    );
}

/// The shipped events with one more: every year, everyone raises `madness` a grade, and a line says so.
fn events_with_fair_years() -> &'static ck_economy::EventRules {
    let mut text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/game/events/character-events.toml"),
    )
    .expect("the shipped events");
    text += "\n[[event]]\nid = \"tmp_fair_years\"\ntrigger = \"yearly\"\nwhen = {}\nchance = 1.0\neffects = [{ raise_grade = \"madness\" }]\ntext = \"{name} lost their wits ({grade}).\"\n";
    support::events_from(&text)
}

#[test]
fn an_excluded_trait_is_not_gained_during_the_start_and_leaves_no_effect_or_line() {
    let events = events_with_fair_years();
    let s =
        started_with("fair", 1, 100_000, false, &data(), (shipped(), events)).expect("the start");
    let lines_of = |c: &Character| -> Vec<String> {
        s.env
            .logs
            .read(c.id)
            .expect("a log")
            .into_iter()
            .map(|line| line.text)
            .collect()
    };
    let figures = [
        "Zzking", "Zzqueen", "Zzprince", "Zzabbot", "Zzking2", "isle1", "isle2", "hermit1",
    ];
    for name in figures {
        let c = live(&s, name);
        assert_eq!(c.grade("madness"), 0, "{name} is never given madness");
        assert!(
            !lines_of(&c)
                .iter()
                .any(|line| line.contains("lost their wits")),
            "{name} has no line of gaining it: {:?}",
            lines_of(&c)
        );
    }
    // The event is live for anyone who is not a real figure: generated characters lose their wits, and the lines say so.
    let others: Vec<&Character> = s
        .store
        .iter()
        .filter(|c| c.life.byname.is_empty() && !figures.contains(&c.name.as_str()))
        .collect();
    assert!(
        others.iter().any(|c| c.grade("beauty") > 0),
        "a generated character gains it"
    );
    assert!(others.iter().any(|c| {
        lines_of(c)
            .iter()
            .any(|line| line.contains("lost their wits"))
    }));
}

#[test]
fn an_authored_genitive_and_patronymic_take_precedence_and_a_generated_characters_come_from_the_pool()
 {
    let mut figures = figures();
    for f in &mut figures {
        match f.key {
            "king" => {
                f.genitive = "Zzkings";
                f.patronymic = "mac Zzfather";
            }
            "queen" => f.patronymic = "ingen Zzdad",
            _ => {}
        }
    }
    let s = started_on("names", 1, 400, false, &data_with(EXCLUDED, &figures)).expect("the start");
    let (king, queen, prince) = (
        live(&s, "Zzking"),
        live(&s, "Zzqueen"),
        live(&s, "Zzprince"),
    );
    let shown = |c: &Character| {
        let father = support::record(&s.store, &s.env, c.father);
        ck_characters::cycle::shown_name(c, (c.father != UNKNOWN).then_some(&father), shipped())
    };
    // The figure's own patronymic, whatever the pool would derive.
    assert!(
        shown(&king).starts_with("Zzking mac Zzfather"),
        "{}",
        shown(&king)
    );
    assert!(
        shown(&queen).starts_with("Zzqueen ingen Zzdad"),
        "{}",
        shown(&queen)
    );
    // Its authored genitive is the one its children's patronymics use: the pool's word, the father's authored genitive.
    assert_eq!(shown(&prince), "Zzprince mac Zzkings");
    // A generated character whose father is a generated one with a pool name takes the pool's genitive.
    let pool = &shipped().pools["irish"].men;
    let generated = s
        .store
        .iter()
        .filter(|c| {
            c.sex == Sex::Male
                && c.life.pool == "irish"
                && c.father != UNKNOWN
                && c.life.patronymic.is_empty()
        })
        .find_map(|c| {
            let father = support::record(&s.store, &s.env, c.father);
            let (_, genitive) = pool.iter().find(|(name, _)| *name == father.name)?;
            (father.life.genitive.is_empty())
                .then(|| (c.clone(), father.name.clone(), genitive.clone()))
        })
        .expect("a generated son of a generated father with a pool name");
    assert!(
        shown(&generated.0).contains(&format!("mac {}", generated.2)),
        "{} of {}",
        shown(&generated.0),
        generated.1
    );
    // A line that names a real figure gives its authored patronymic: the strand lines of the burn-in name both ends.
    let mut named_lines = 0;
    for c in s.store.iter() {
        for line in s.env.logs.read(c.id).expect("a log") {
            named_lines += usize::from(line.text.contains("Zzking mac Zzfather"));
        }
    }
    assert!(
        named_lines > 0,
        "a log line names the king with his authored patronymic"
    );
}

/// A small world: a seat of `heads` noble heads with peasants beside them (so a relative of a lay character stays in his pop), and a court.
fn tiny_pops(heads: u64) -> Pops {
    Pops::new()
        .pop("tiny", 0, "noble", "mumu", heads)
        .pop("tiny", 1, "peasant", "mumu", 100_000)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "three forced lives of one wife (dies, lives, dies at a delivery), each run and read in turn"
)]
fn a_spouse_who_dies_before_the_opening_never_charges_a_head_and_one_who_lives_does() {
    let solo = vec![Fig {
        name: "Zzsolo",
        house: "ui_s",
        ..fig("solo", "tiny", "noble", "m", 745)
    }];
    // No child is conceived: the only question is the wife. A forced natural death on her 16th birthday takes her before the opening (the
    // pop has one head and one resident, and must stay so); a wife who lives takes the head the pop has to be raised for.
    let dies = certain(|l| {
        l.conception.monthly_ppb = 0;
        l.death.certain_at = 16;
        for ages in l.marriageable.values_mut() {
            ages.m = 14;
            ages.f = 14;
        }
    });
    let lives = certain(|l| {
        l.conception.monthly_ppb = 0;
        for ages in l.marriageable.values_mut() {
            ages.m = 14;
            ages.f = 14;
        }
    });
    let run_seed = |lifecycle: &'static ck_economy::LifeCycle, seed: u64| {
        let pops = tiny_pops(1);
        pops.net.set(true);
        let mut env = Env::new("one-head");
        let mut store = CharacterStore::default();
        {
            let mut w =
                support::world_with(&support::rules().events, lifecycle, &mut env, &pops, seed);
            store
                .start_768(&mut w, &data_of(&solo, &[], ""), start_day())
                .expect("the start");
        }
        (store, pops, env)
    };
    let run = |lifecycle: &'static ck_economy::LifeCycle| run_seed(lifecycle, 1);
    let (store, pops, env) = run(dies);
    let wife = live_in(&store, "Zzsolo")
        .spouses
        .iter()
        .map(|id| support::record_in(&store, &env.archive, *id))
        .next()
        .expect("a wife");
    assert!(
        wife.death.is_some(),
        "the forced draw took her before the opening"
    );
    assert_eq!(
        store.pop_count(&locator("tiny", 0)),
        1,
        "only the resident is there"
    );
    assert!(
        pops.raised.borrow().is_empty(),
        "a spouse who is not there at the opening raises nothing: {:?}",
        pops.raised.borrow()
    );
    // Natural death is possible for her too: the first seed on which she lives.
    let (store, pops, _) = (1..40)
        .map(|seed| run_seed(lives, seed))
        .find(|(store, _, _)| {
            live_in(store, "Zzsolo")
                .spouses
                .iter()
                .any(|id| store.get(*id).is_some())
        })
        .expect("a seed on which she lives");
    let wife = live_in(&store, "Zzsolo")
        .spouses
        .iter()
        .find_map(|id| store.get(*id))
        .expect("a living wife");
    assert_eq!(wife.pop, locator("tiny", 0));
    // She is there at the opening (with her living parents, who are made in her pop), and the pop is raised by the people it holds beyond its
    // one head: she is charged, and exactly that.
    let present = store.pop_count(&locator("tiny", 0)) as u64;
    assert!(present >= 2, "{present}");
    assert_eq!(
        pops.raised.borrow().get(&locator("tiny", 0)).copied(),
        Some(present - 1),
        "one raise for each person there beyond the one head"
    );
    // A fatal delivery is the same: the wife who dies at it never took the head; only the child, who lives, did.
    let fatal = certain(|l| {
        l.death.childbirth_ppb = ck_economy::genetics::PPB;
        l.conception.twins_ppb = 0;
        for ages in l.marriageable.values_mut() {
            ages.m = 14;
            ages.f = 14;
        }
    });
    let (store, pops, env) = run(fatal);
    let wife = live_in(&store, "Zzsolo")
        .spouses
        .iter()
        .map(|id| support::record_in(&store, &env.archive, *id))
        .next()
        .expect("a wife");
    assert_eq!(wife.life.death_cause.as_deref(), Some("childbirth"));
    let residents = store.pop_count(&locator("tiny", 0)) as u64;
    assert_eq!(
        pops.raised
            .borrow()
            .get(&locator("tiny", 0))
            .copied()
            .unwrap_or(0),
        residents - 1,
        "raised only for the people there: {residents}"
    );
}

fn live_in(store: &CharacterStore, name: &str) -> Character {
    store
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no live {name}"))
        .clone()
}

#[test]
fn at_a_noble_seat_with_no_peasants_a_generated_lord_his_wife_and_an_authored_figures_household_stay_in_the_noble_pop()
 {
    let slots = r#"
[[slot]]
key = "crag_lord"
location = "crag"
[[slot.member]]
role = "lord"
count = 1
pop = "noble"
sex = "m"
[[slot.member]]
role = "spouse_of_lord"
count = 1
pop = "noble"
sex = "f"
"#;
    // An authored figure at the seat, with no wife named: his household generates a spouse and children (every month a conception).
    let figures = vec![Fig {
        name: "Zzcraggan",
        house: "ui_c",
        ..fig("craggan", "crag", "noble", "m", 745)
    }];
    let lifecycle = certain(|l| {
        for ages in l.marriageable.values_mut() {
            ages.m = 14;
            ages.f = 14;
        }
    });
    let pops = pops(400);
    pops.net.set(true);
    let mut env = Env::new("lord-wife");
    let mut store = CharacterStore::default();
    {
        let mut w = support::world_with(&support::rules().events, lifecycle, &mut env, &pops, 1);
        store
            .start_768(&mut w, &data_of(&figures, &[], slots), start_day())
            .expect("the start");
    }
    // The step-5 generated lord and his generated wife, who fills the spouse slot, are both in the seat's noble pop.
    let lord = store
        .iter()
        .find(|c| c.life.role == "lord")
        .expect("a lord")
        .clone();
    assert_eq!(lord.pop, locator("crag", 0));
    let wife = lord
        .spouses
        .iter()
        .filter_map(|id| store.get(*id))
        .find(|c| c.life.role == "spouse_of_lord")
        .expect("his wife");
    assert_eq!(
        (wife.life.slot_key.as_str(), wife.life.slot_kind.as_str()),
        ("crag_lord", "spouse_of_lord")
    );
    assert_eq!(
        wife.pop,
        locator("crag", 0),
        "she lives where the slot puts her, with her husband"
    );
    assert!(wife.spouses.contains(&lord.id));
    // The authored figure's generated spouse and children live in his noble pop too.
    let craggan = live_in(&store, "Zzcraggan");
    // Her fate is the household's (with a conception every month she may die at a delivery): live or archived, her pop is his.
    let spouse = craggan
        .spouses
        .iter()
        .map(|id| support::record_in(&store, &env.archive, *id))
        .next()
        .expect("a generated spouse");
    assert_eq!(
        spouse.pop, craggan.pop,
        "his generated spouse is in his noble pop"
    );
    assert!(!craggan.children.is_empty(), "the household has children");
    for child in craggan.children.iter().filter_map(|id| store.get(*id)) {
        assert_eq!(child.pop, craggan.pop, "his children stay in his noble pop");
    }
    assert_eq!(craggan.pop, locator("crag", 0));
    assert!(
        pops.moves.borrow().is_empty(),
        "no head moved: nobody was sent elsewhere"
    );
}

#[test]
fn a_craft_grade_a_figure_is_never_given_is_not_reached_by_advancement_and_leaves_no_line() {
    let mut events = support::rules().events.clone();
    events.craft_rise.rise_ppb = u32::MAX;
    let events: &'static ck_economy::EventRules = Box::leak(Box::new(events));
    let mut figures = men_of("crafter", "court", "m", 6, 738);
    for f in &mut figures {
        f.forced = "\"poet\"";
    }
    // The first three are never given `poet` at B or above; the others are the control.
    let (strict, free) = figures.split_at(3);
    let mut all: Vec<Fig> = strict.to_vec();
    all.extend(free.iter().cloned());
    let (research, load) = files(&all, &[], &[], "", EXCLUDED);
    let research = research.replacen(
        "forced = [\"poet\"]",
        "forced = [\"poet\"]\nexcluded = [\"poet:B\"]",
        3,
    );
    let data = FigureData::parse(&research, &load).expect("the data loads");
    let s =
        started_with("crafts", 1, 100_000, false, &data, (shipped(), events)).expect("the start");
    let rose = |c: &Character| -> usize {
        s.env
            .logs
            .read(c.id)
            .expect("a log")
            .iter()
            .filter(|line| line.text.contains("grew in skill as"))
            .count()
    };
    let (mut control_rose, mut control_grades) = (0, 0);
    for n in 0..6 {
        let c = live(&s, &format!("crafter{n}"));
        if n < 3 {
            assert_eq!(c.grade("poet"), 1, "crafter{n} stays at C");
            assert_eq!(rose(&c), 0, "no line of a rise it never had");
        } else {
            control_rose += rose(&c);
            control_grades += usize::from(c.grade("poet") > 1);
        }
    }
    assert!(
        control_rose > 0 && control_grades > 0,
        "advancement is live for those not excluded: {control_rose} {control_grades}"
    );
}

#[test]
fn strand_lines_name_a_character_by_its_derived_name_with_the_authored_genitive_or_the_pools() {
    let mut figures = figures();
    for f in &mut figures {
        if f.key == "king" {
            f.genitive = "Zzkings";
        }
    }
    let s =
        started_on("derived", 1, 400, false, &data_with(EXCLUDED, &figures)).expect("the start");
    let (mut authored, mut pool) = (0, 0);
    let pool_genitives: Vec<String> = shipped().pools["irish"]
        .men
        .iter()
        .map(|(_, g)| g.clone())
        .collect();
    for c in s.store.iter() {
        for line in s.env.logs.read(c.id).expect("a log") {
            // A line that names the king's son gives the patronymic with the king's authored genitive.
            authored += usize::from(line.text.contains("Zzprince mac Zzkings"));
            // A line that names a generated character gives its pool patronymic: `mac` and a genitive of the pool.
            pool += usize::from(
                pool_genitives
                    .iter()
                    .any(|g| line.text.contains(&format!(" mac {g} "))),
            );
        }
    }
    assert!(
        authored > 0,
        "a line names the king's son with the authored genitive"
    );
    assert!(
        pool > 0,
        "a line names a generated character with the pool's genitive"
    );
}

#[test]
fn a_spouse_a_slot_names_is_made_in_her_husbands_pop_even_where_his_relatives_live_elsewhere() {
    // A ruler in the monk pop of `hermit` (a monastic place: its generated relatives live at the nearest peasant pop, `farm`), and the slot
    // that names his wife in the same pop. The household generates her in step 4 and step 5 binds her to the slot: she lives with him, and
    // her husband's own (generated) parents are at the farm.
    let slots = r#"
[[slot]]
key = "hermit_seat"
location = "hermit"
[[slot.member]]
role = "spouse_of_ruler"
count = 1
pop = "monk"
sex = "f"
"#;
    let ruler = vec![Fig {
        name: "Zzabbot",
        house: "ui_a",
        office: "ruler",
        ..fig("abbot", "hermit", "monk", "m", 738)
    }];
    let lifecycle = certain(|l| {
        l.death.childbirth_ppb = 0;
        l.conception.monthly_ppb = 0;
    });
    for seed in 1..=6 {
        let s = started_with(
            "monastic-wife",
            seed,
            400,
            true,
            &data_of(&ruler, &[], slots),
            (lifecycle, &support::rules().events),
        )
        .expect("the start");
        let ruler = live(&s, "Zzabbot");
        let wife = ruler
            .spouses
            .iter()
            .find_map(|id| s.store.get(*id))
            .unwrap_or_else(|| panic!("seed {seed}: no living wife"));
        assert!(wife.life.household_complete, "seed {seed}: made in step 4");
        assert_eq!(
            (
                wife.life.slot_key.as_str(),
                wife.life.slot_kind.as_str(),
                wife.sex
            ),
            ("hermit_seat", "spouse_of_ruler", Sex::Female)
        );
        assert_eq!(ruler.pop, locator("hermit", 0));
        assert_eq!(
            wife.pop, ruler.pop,
            "seed {seed}: she lives with her husband"
        );
        // Her head is charged where she lives: the pop holds the two of them (and the net raised it for the people it holds).
        assert!(s.store.pop_count(&locator("hermit", 0)) >= 2);
        // The ordinary relative destination holds for the rest of the household: the ruler's parents are at the farm.
        let parents: Vec<_> = [ruler.father, ruler.mother]
            .into_iter()
            .filter_map(|id| s.store.get(id))
            .collect();
        assert!(
            parents.iter().all(|p| p.pop == locator("farm", 0)),
            "seed {seed}"
        );
    }
}

#[test]
fn the_relatives_curve_belongs_to_the_start_and_an_ancestor_made_in_play_keeps_the_conditioned_stream()
 {
    // The same generate call outside the start, with the flag on and off: the parents' deaths are the same (CK-369's `ancestor` stream and its
    // conditioned windows), whatever the data's flag says; inside the start they come from the `relative` stream and may differ.
    let (mut same, mut differ_in_start) = (0, 0);
    for seed in 1..=40_u64 {
        let outcome = |lifecycle: &'static ck_economy::LifeCycle, in_start: bool| {
            let pops = pops(400);
            let mut env = Env::new("in-play");
            let mut store = CharacterStore::default();
            if in_start {
                store.begin_start(start_day());
            }
            let mut w = world(lifecycle, &mut env, &pops, seed);
            let request = ck_characters::generator::GenerateRequest::new(
                locator("court", 0),
                Sex::Female,
                ck_characters::generator::Birth::On(date(747, 1, 1)),
                "candidate",
                date(769, 3, 1),
            );
            let made = store.generate(&mut w, &request).expect("a candidate");
            let child = support::record_in(&store, &env.archive, made.id);
            [child.father, child.mother]
                .into_iter()
                .map(|id| support::record_in(&store, &env.archive, id).death)
                .collect::<Vec<_>>()
        };
        let (on, off) = (
            outcome(shipped(), false),
            outcome(without_relative_deaths(), false),
        );
        assert_eq!(on, off, "seed {seed}: in play the flag changes nothing");
        same += 1;
        differ_in_start +=
            usize::from(outcome(shipped(), true) != outcome(without_relative_deaths(), true));
    }
    assert_eq!(same, 40);
    assert!(
        differ_in_start > 0,
        "in the start the flag selects the other stream"
    );
}
