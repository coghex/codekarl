//! Character event data contracts (CK-363): the shipped files load, and each validation rule refuses the data it names.

use crate::{
    DataError, EventRules, Rules,
    events::{Effect, Factor, Kind, Registry, StressFactor, Trigger},
};
use std::{path::Path, sync::OnceLock};

fn asset(name: &str) -> String {
    // Read at run time, not with `include_str!`, so a crate built without the assets directory still compiles.
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/game/")
            .join(name),
    )
    .expect("shipped data")
}
fn events_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| asset("events/character-events.toml"))
}
fn attributes_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| asset("events/attributes-skills.toml"))
}
fn shipped_rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .expect("shipped rules")
    })
}
fn parse(events: &str, attributes: &str) -> Result<EventRules, DataError> {
    let rules = shipped_rules();
    let pop_types: Vec<&str> = rules.pop_types.iter().map(|p| p.id.as_str()).collect();
    EventRules::parse(
        "character-events.toml",
        events,
        "attributes-skills.toml",
        attributes,
        &Registry {
            peoples: &rules.peoples,
            genetics: &rules.genetics,
            pop_types: &pop_types,
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
fn refused_events(from: &str, to: &str, entry: &str, problem: &str) {
    let error = parse(&edit(events_text(), from, to), attributes_text()).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert_eq!(error.file, "character-events.toml");
}
fn refused_attributes(from: &str, to: &str, entry: &str, problem: &str) {
    let error = parse(events_text(), &edit(attributes_text(), from, to)).unwrap_err();
    assert!(
        error.entry.contains(entry) && error.problem.contains(problem),
        "expected `{entry}` / `{problem}`, got {error:?}"
    );
    assert_eq!(error.file, "attributes-skills.toml");
}

#[test]
fn the_shipped_files_load_with_what_they_state() {
    let rules = parse(events_text(), attributes_text()).expect("shipped events");
    assert_eq!(rules.catalog.len(), 57);
    assert_eq!(rules.events.len(), 56);
    assert_eq!(rules.events[0].id, "birth_of_child_joy");
    assert_eq!(
        rules.events[0].trigger,
        Trigger::Happening("child_born".into())
    );
    assert_eq!(rules.events[0].chance_ppb, 250_000_000);
    assert_eq!(
        rules.events[0].factors,
        vec![Factor::Humour(0), Factor::Stress(StressFactor::Lift)]
    );
    assert_eq!(
        rules.events[0].effects,
        vec![Effect::LowerGrade("melancholy".into())]
    );
    assert_eq!(rules.deeds.len(), 9);
    assert_eq!(
        rules.deed("veteran").expect("veteran").thresholds,
        [3, 6, 10, 15, 21, 28]
    );
    assert_eq!(rules.craft_rise.rise_ppb, 50_000_000);
    assert_eq!(rules.skills.list.len(), 11);
    assert_eq!(rules.skills.train_from, 7);
    assert_eq!(rules.skills.gain_rate, 3000);
    assert_eq!(
        rules.skills.age_factor,
        vec![(0, 1000), (14, 300), (25, 100)]
    );
    assert_eq!(rules.keep_while.len(), 8);
    assert_eq!(rules.catalog["ascetic"].kind, Kind::Calling);
    assert_eq!(rules.culture_group["ulaid"], "gaelic");
    // Every key the data and genetics name is in the catalog.
    for event in &rules.events {
        for key in event.when.has.iter().chain(&event.when.lacks) {
            assert!(rules.catalog.contains_key(key), "{key}");
        }
    }
    for name in shipped_rules()
        .genetics
        .mendelian()
        .keys()
        .chain(shipped_rules().genetics.graded().keys())
    {
        assert!(rules.catalog.contains_key(name), "{name}");
    }
}

#[test]
fn an_effect_the_kind_does_not_allow_is_refused() {
    // A rule that grants a deed, removes a deed or craft, or lowers or raises a non-mind trait.
    refused_events(
        r#"effects = [{ grant = "fennid" }]"#,
        r#"effects = [{ grant = "veteran" }]"#,
        "effects[0]",
        "does not apply",
    );
    refused_events(
        r#"effects = [{ remove = "fennid" }]"#,
        r#"effects = [{ remove = "poet" }]"#,
        "effects[0]",
        "does not apply",
    );
    refused_events(
        r#"effects = [{ remove = "fennid" }]"#,
        r#"effects = [{ remove = "veteran" }]"#,
        "effects[0]",
        "does not apply",
    );
    refused_events(
        r#"effects = [{ lower_grade = "melancholy" }]"#,
        r#"effects = [{ lower_grade = "poet" }]"#,
        "effects[0]",
        "does not apply",
    );
    refused_events(
        r#"effects = [{ raise_grade = "recluse" }]"#,
        r#"effects = [{ raise_grade = "beauty" }]"#,
        "effects[0]",
        "does not apply",
    );
    refused_events(
        r#"effects = [{ grant = "ascetic" }]"#,
        r#"effects = [{ grant = "melancholy" }]"#,
        "effects[0]",
        "does not apply",
    );
}

#[test]
fn counts_name_a_counter_and_add_at_least_one() {
    refused_events(
        r#"{ count = "battles_survived", add = 1 }"#,
        r#"{ count = "no_such_counter", add = 1 }"#,
        "count",
        "not a counter",
    );
    refused_events(
        r#"{ count = "battles_survived", add = 1 }"#,
        r#"{ count = "battles_survived", add = 0 }"#,
        "add",
        "at least 1",
    );
    refused_events(
        r#"{ count = "battles_survived", add = 1 }"#,
        r#"{ count = "battles_survived", add = -1 }"#,
        "add",
        "non-negative",
    );
}

#[test]
fn unknown_names_and_malformed_values_are_refused() {
    refused_events(
        r#"{ humour = "blood" }, { stress = "lift" }]"#,
        r#"{ humour = "bile" }, { stress = "lift" }]"#,
        "humour",
        "not a humour",
    );
    refused_events(
        r#"{ stress = "lift" }]
effects = [{ lower_grade = "melancholy" }]
text = "The birth"#,
        r#"{ colour = "red" }]
effects = [{ lower_grade = "melancholy" }]
text = "The birth"#,
        "factors[1]",
        "unknown factor",
    );
    refused_events(
        r"effects = [{ stress = -5 }]",
        r"effects = [{ shout = 5 }]",
        "effects[0]",
        "unknown effect",
    );
    refused_events(
        r#"when = { has = ["melancholy"] }"#,
        r#"when = { has = ["melancholy"], mood = "low" }"#,
        "when.mood",
        "unknown key",
    );
    refused_events(
        r#"when = { has = ["melancholy"] }"#,
        r#"when = { has = ["nonsense"] }"#,
        "when.has",
        "not a key of the catalog",
    );
    refused_events(
        r#"when = { has = ["melancholy"] }"#,
        r#"when = { has = ["melancholy"], lacks_land = true }"#,
        "when.lacks_land",
        "unknown key",
    );
    refused_events(
        r#"when = { has = ["melancholy"] }"#,
        r#"when = { has = ["melancholy"], pop_luxuries_min = 500 }"#,
        "when.pop_luxuries_min",
        "unknown key",
    );
    refused_events(
        r#"chance = 0.25
factors = [{ humour = "blood" }, { stress = "lift" }]"#,
        "chance = 1.5\nfactors = [{ humour = \"blood\" }, { stress = \"lift\" }]",
        "chance",
        "0..1",
    );
    refused_events(
        r#"chance = 0.25
factors = [{ humour = "blood" }, { stress = "lift" }]"#,
        "chance = \"high\"\nfactors = [{ humour = \"blood\" }, { stress = \"lift\" }]",
        "chance",
        "expected a number",
    );
    refused_events(
        r#"trigger = "happening:marriage""#,
        r#"trigger = "monthly""#,
        "trigger",
        "yearly",
    );
    refused_events(
        r#"id = "marriage_recluse_lift""#,
        r#"id = "marriage""#,
        "event.marriage",
        "duplicate",
    );
    refused_events(
        r#"skill = "oratory", craft = true }, { pop_type = { monk = 3"#,
        r#"skill = "swimming", craft = true }, { pop_type = { monk = 3"#,
        "skill",
        "not in `[skills].list`",
    );
    refused_events(
        r#"{ grade = "melancholy", by = [1, 1, 1, 1, 1, 0.7, 0.5] }"#,
        r#"{ grade = "fennid", by = [1, 1, 1, 1, 1, 0.7, 0.5] }"#,
        "grade",
        "not graded",
    );
    refused_events(
        r#"{ grade = "melancholy", by = [1, 1, 1, 1, 1, 0.7, 0.5] }"#,
        r#"{ grade = "melancholy", by = [1, 1, 1] }"#,
        "by",
        "seven",
    );
}

#[test]
fn a_placeholder_no_effect_can_set_is_refused() {
    refused_events(
        r#"text = "{name} married.""#,
        r#"text = "{name} married {trait}.""#,
        "text",
        "no effect of this event can set",
    );
    refused_events(
        r#"text = "{name} married.""#,
        r#"text = "{name} married {grade}.""#,
        "text",
        "no effect of this event can set",
    );
    refused_events(
        r#"text = "{name} married.""#,
        r#"text = "{name} married {count}.""#,
        "text",
        "no effect of this event can set",
    );
    refused_events(
        r#"text = "{name} married.""#,
        r#"text = "{name} married {spouse}.""#,
        "text",
        "unknown placeholder",
    );
    refused_events(
        r#"text = "{name} married.""#,
        r#"text = "{name married.""#,
        "text",
        "unclosed",
    );
}

#[test]
fn precedence_only_removes_callings_stations_and_backgrounds() {
    for key in ["poet", "veteran", "melancholy", "beauty"] {
        refused_attributes(
            "[keep_while.hospitaller]",
            &format!("[keep_while.{key}]\nhas = [\"ascetic\"]\n\n[keep_while.hospitaller]"),
            &format!("keep_while.{key}"),
            "only those can be removed",
        );
    }
}

#[test]
fn deeds_and_crafts_and_skills_are_checked() {
    refused_attributes(
        "thresholds = [3, 6, 10, 15, 21, 28]   # owner",
        "thresholds = [3, 6, 6, 15, 21, 28]   # owner",
        "thresholds",
        "strictly increasing",
    );
    refused_attributes(
        "thresholds = [3, 6, 10, 15, 21, 28]   # owner",
        "thresholds = [3, 6, 10, 15, 21]   # owner",
        "thresholds",
        "six",
    );
    refused_attributes(
        "[deeds.veteran]",
        "[deeds.poet]",
        "deeds.poet",
        "not a deed",
    );
    refused_attributes(
        r#"poet = "oratory""#,
        r#"poet = "swimming""#,
        "craft_rise.skills.poet",
        "not in `[skills].list`",
    );
    refused_attributes(
        r"skill_min = { oratory = 8 }",
        r"skill_min = { swimming = 8 }",
        "skill_min",
        "not in `[skills].list`",
    );
    refused_attributes(r"rise = 0.05", r"rise = 1.5", "craft_rise.rise", "0..1");
    refused_attributes(
        r#"oratory = "wit""#,
        r#"oratory = "grace""#,
        "skills.aptitude.oratory",
        "not a stat",
    );
    refused_attributes(
        r"[skills.use.noble.m]",
        r"[skills.use.knight.m]",
        "skills.use.knight",
        "not a pop type",
    );
    refused_attributes(
        r"noble_m = { balanced = 40, martial = 30",
        r"noble_m = { balanced = 40, jousting = 30",
        "noble_m",
        "not a focus",
    );
    refused_attributes(
        r"age_factor = [[0, 1.0], [14, 0.3], [25, 0.1]]",
        r"age_factor = [[1, 1.0], [14, 0.3], [25, 0.1]]",
        "age_factor",
        "start at 0",
    );
}

#[test]
fn the_catalog_is_checked_against_itself_and_genetics() {
    refused_attributes(
        r#"beauty = { kind = "inherited_trait", graded = true }"#,
        r#"beauty = { kind = "inherited_trait", graded = false }"#,
        "catalog.beauty",
        "disagrees with genetics",
    );
    refused_attributes(
        r#"beauty = { kind = "inherited_trait", graded = true }"#,
        r#"beauty = { kind = "oddity", graded = true }"#,
        "catalog.beauty.kind",
        "not a kind",
    );
    refused_attributes(
        r#"fennid = { kind = "background", graded = false }"#,
        r#"fennid = { kind = "background", graded = true }"#,
        "catalog.fennid",
        "ungraded",
    );
    refused_attributes(
        r#"drunkard = { kind = "mind_trait", graded = true, stress_moves = true }"#,
        r#"drunkard = { kind = "craft", graded = true }"#,
        "catalog.drunkard",
        "slice 3",
    );
}

#[test]
fn precedence_scans_in_the_authored_catalog_order() {
    let rules = parse(events_text(), attributes_text()).expect("shipped events");
    assert_eq!(rules.catalog_order.len(), 57);
    assert_eq!(rules.catalog_order[0], "beauty");
    let order: Vec<&str> = rules
        .keep_while
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    // The catalog writes hospitaller before fennid; alphabetical order would put fennid first.
    assert_eq!(
        order,
        [
            "ascetic",
            "cele_de",
            "preacher",
            "soul_friend",
            "saintly",
            "hospitaller",
            "fennid",
            "rigfennid"
        ]
    );
}

#[test]
fn a_placeholder_set_only_after_a_death_is_refused() {
    refused_events(
        r#"text = "{name} married.""#,
        r#"text = "{name} married {grade}.""#,
        "text",
        "no effect of this event can set",
    );
    let after_death = edited_events(
        r#"effects = [{ stress = -10 }]
text = "{name} married.""#,
        r#"effects = [{ die = "test" }, { raise_grade = "melancholy" }]
text = "{name} married {grade}.""#,
    );
    let error = parse(&after_death, attributes_text()).unwrap_err();
    assert!(
        error.problem.contains("no effect of this event can set"),
        "{error:?}"
    );
    // Before the death it is settable.
    let before_death = edited_events(
        r#"effects = [{ stress = -10 }]
text = "{name} married.""#,
        r#"effects = [{ raise_grade = "melancholy" }, { die = "test" }]
text = "{name} married {grade}.""#,
    );
    parse(&before_death, attributes_text()).expect("a setter before the death");
}

fn edited_events(from: &str, to: &str) -> String {
    edit(events_text(), from, to)
}

#[test]
fn the_skill_parameters_have_a_validated_range() {
    refused_attributes(
        "gain_rate = 3.0",
        "gain_rate = 100000.001",
        "skills.gain_rate",
        "at most 100000",
    );
    refused_attributes("on = 1.5", "on = 10.001", "skills.focus.on", "at most 10");
    refused_attributes(
        "rate = 0.05 ",
        "rate = 1.001 ",
        "home_education",
        "at most 1",
    );
    refused_attributes(
        "scholarship_per_point = 0.02",
        "scholarship_per_point = 1.5",
        "home_education",
        "at most 1",
    );
    refused_attributes("[14, 0.3]", "[14, 1.5]", "age_factor", "at most 1");
    parse(
        events_text(),
        &edit(attributes_text(), "gain_rate = 3.0", "gain_rate = 100000.0"),
    )
    .expect("the bound itself");
}

#[test]
fn a_quoted_catalog_key_reads_in_the_same_order() {
    // `"hospitaller" = { ... }` is the same TOML key as `hospitaller = { ... }`: the order comes from the parsed source.
    let quoted = edit(
        attributes_text(),
        "hospitaller = { kind",
        "\"hospitaller\" = { kind",
    );
    let rules = parse(events_text(), &quoted).expect("a quoted key");
    let order: Vec<&str> = rules
        .keep_while
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(
        order,
        [
            "ascetic",
            "cele_de",
            "preacher",
            "soul_friend",
            "saintly",
            "hospitaller",
            "fennid",
            "rigfennid"
        ]
    );
    assert_eq!(rules.catalog.len(), 57);
}
