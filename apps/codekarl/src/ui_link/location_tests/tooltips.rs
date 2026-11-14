use super::*;
use crate::ui_link::widget_witness_tests::points;
use ck_window::PointerPosition;

#[test]
fn built_icon_buttons_and_tabs_have_their_spec_tooltip_labels() {
    let mut rig = rig(&[
        r#"time(1, 1); select_place('rowan', 'Rowan'); tabs_by_title.Rowan:select('pops', true)
for _, key in ipairs({"location", "pops"}) do note_tooltip("places_" .. key, bar_buttons[key].key) end
for _, key in ipairs({"location", "pops"}) do note_tooltip_kind("places_" .. key, bar_buttons[key].key) end
for _, key in ipairs({"stockpile", "yield", "elevation"}) do
  note_tooltip("mode_" .. key, mode_buttons[key].key); note_tooltip_kind("mode_" .. key, mode_buttons[key].key)
end
for _, key in ipairs({"rewind", "fast_forward"}) do note_tooltip("time_" .. key, time_buttons[key].key) end
for _, key in ipairs({"rewind", "fast_forward"}) do note_tooltip_kind("time_" .. key, time_buttons[key].key) end
for index = 1, 5 do note_tooltip("speed_" .. index, "speed_" .. index) end
for index = 1, 5 do note_tooltip_kind("speed_" .. index, "speed_" .. index) end
for _, key in ipairs({"cards", "rows"}) do note_tooltip("look_" .. key, look_buttons[key].key) end
for _, key in ipairs({"cards", "rows"}) do note_tooltip_kind("look_" .. key, look_buttons[key].key) end
note_tooltip("location_full", tabs_by_title.Rowan.buttons[1].key)
note_tooltip_kind("location_full", tabs_by_title.Rowan.buttons[1].key)
bar_buttons.location.on_click()
for index, tab in ipairs(tabs_by_title.ledger.items) do note_tooltip("ledger_" .. tab.key, tabs_by_title.ledger.buttons[index].key) end
for index, tab in ipairs(tabs_by_title.ledger.items) do note_tooltip_kind("ledger_" .. tab.key, tabs_by_title.ledger.buttons[index].key) end
announce_main_tooltips()"#,
        r#"time(1, 1, true); note_tooltip("time_pause", time_buttons.pause.key); note_tooltip_kind("time_pause", time_buttons.pause.key)"#,
        r#"note("location_tier " .. tabs_by_title.Rowan.tier); note_tooltip("location_short", tabs_by_title.Rowan.buttons[1].key); note_tooltip_kind("location_short", tabs_by_title.Rowan.buttons[1].key)"#,
    ]);
    rig.opened();
    let recorded = notes(&rig);
    assert_tooltip_labels(&recorded, BUILT_ICON_TOOLTIP_LABELS);
    assert_tooltip_modes(&recorded, BUILT_ICON_TOOLTIP_MODES);
    assert_tooltip_kinds(&recorded, BUILT_ICON_TOOLTIP_KINDS);
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip location_full <none>")
    );

    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip time_pause play part=-")
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip_kind time_pause label")
    );

    rig.deliver_fixture_frame_at(640, 900, 2);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "location_tier 2" || line == "location_tier 3")
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip location_short location part=-")
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_kind location_short label")
    );
}

const BUILT_ICON_TOOLTIP_LABELS: &[(&str, &str)] = &[
    ("places_location", "places"),
    ("places_pops", "pops"),
    ("time_rewind", "slower"),
    ("time_fast_forward", "faster"),
    ("speed_1", "speed 1"),
    ("speed_2", "speed 2"),
    ("speed_3", "speed 3"),
    ("speed_4", "speed 4"),
    ("speed_5", "speed 5"),
    ("look_cards", "cards"),
    ("look_rows", "list"),
    ("ledger_locations", "places"),
    ("ledger_pops", "pops"),
];

const BUILT_ICON_TOOLTIP_MODES: &[(&str, &str)] = &[
    ("mode_stockpile", "stockpile map"),
    ("mode_yield", "yield map"),
    ("mode_elevation", "elevation map"),
];

const BUILT_ICON_TOOLTIP_KINDS: &[(&str, &str)] = &[
    ("places_location", "label"),
    ("places_pops", "label"),
    ("time_rewind", "label"),
    ("time_fast_forward", "label"),
    ("speed_1", "label"),
    ("speed_2", "label"),
    ("speed_3", "label"),
    ("speed_4", "label"),
    ("speed_5", "label"),
    ("look_cards", "label"),
    ("look_rows", "label"),
    ("ledger_locations", "label"),
    ("ledger_pops", "label"),
    ("mode_stockpile", "lockable"),
    ("mode_yield", "lockable"),
    ("mode_elevation", "lockable"),
    ("date", "lockable"),
];

fn assert_tooltip_kinds(recorded: &[String], expected: &[(&str, &str)]) {
    for (name, kind) in expected {
        assert!(
            recorded
                .iter()
                .any(|line| line == &format!("tooltip_kind {name} {kind}")),
            "wrong source kind for {name}: {recorded:?}"
        );
    }
}

fn assert_tooltip_labels(recorded: &[String], expected: &[(&str, &str)]) {
    for (name, words) in expected {
        assert!(
            recorded
                .iter()
                .any(|line| line == &format!("tooltip {name} {words} part=-")),
            "missing {name}={words}: {recorded:?}"
        );
    }
}

fn assert_tooltip_modes(recorded: &[String], expected: &[(&str, &str)]) {
    for (name, title) in expected {
        assert!(
            recorded
                .iter()
                .any(|line| line.starts_with(&format!("tooltip {name} title={title}|"))),
            "missing {name} title={title}: {recorded:?}"
        );
    }
}

fn point_note(rig: &Rig, prefix: &str) -> (f64, f64) {
    let line = notes(rig)
        .into_iter()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no point note {prefix}: {:?}", notes(rig)));
    let mut values = line[prefix.len()..]
        .split_whitespace()
        .map(|value| value.parse::<f64>().unwrap());
    (values.next().unwrap(), values.next().unwrap())
}

#[test]
fn card_tooltip_dispatch_returns_herds_and_plain_pop_cards_stay_silent() {
    let mut rig = rig(&[r#"VIEWS[1].cards.buildings[1].herds = {
  {species = "cattle", head = 27, maximum = 36},
  {species = "pigs", head = 9, maximum = 12},
  {species = "sheep", head = 10, maximum = 20},
}
time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("buildings", true)
local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
local P = require("lib.ui").P
local building = card_object.items[1]
local cattle_x = 5 + 4
local pigs_x = 5 + 11 + font.layout("27/36").width + 10
local left, top = card_object.origin_x + card_object.x, card_object.origin_y + card_object.y + card_object.tops[1]
note(string.format("cattle_point %g %g", left + cattle_x * P, top + 42 * P))
note(string.format("pigs_point %g %g", left + (pigs_x + 4) * P, top + 42 * P))
local cattle_count_right = 16 + font.layout("27/36").width
note(string.format("gap_point %g %g", left + (cattle_count_right + 3) * P, top + 42 * P))
local content, part = building.tooltip(cattle_x + 4, 42)
note("card_cattle_source title=" .. content.title .. " part=" .. tostring(part))
content, part = building.tooltip(pigs_x + 4, 42)
note("card_pigs_source title=" .. content.title .. " part=" .. tostring(part))"#]);
    rig.opened();

    let cattle = point_note(&rig, "cattle_point ");
    let pigs = point_note(&rig, "pigs_point ");
    assert!(
        rig.key_at(cattle.0, cattle.1).is_some(),
        "the cattle icon is inside its card hit key"
    );
    assert!(
        rig.key_at(pigs.0, pigs.1).is_some(),
        "the pig icon is inside its card hit key"
    );
    rig.feed(&[PointerEvent::Move(points(cattle.0, cattle.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line == "tooltip_text cattle"),
        "real card pointer dispatch should open the cattle tooltip without a Lua error: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "card_cattle_source title=cattle part=cattle"),
        "the card provider returns cattle with the cattle part"
    );

    let before_pigs = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(pigs.0, pigs.1))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert!(
        !notes(&rig)[before_pigs..]
            .iter()
            .any(|line| line == "tooltip_text pigs"),
        "changing species on one card restarts the delay"
    );
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig)[before_pigs..]
            .iter()
            .any(|line| line == "tooltip_text pigs"),
        "the delayed card tooltip then changes to pigs"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "card_pigs_source title=pigs part=pigs"),
        "the card provider returns pigs with the pigs part"
    );

    let before_gap = notes(&rig).len();
    let gap = point_note(&rig, "gap_point ");
    rig.feed(&[PointerEvent::Move(points(gap.0, gap.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        !notes(&rig)[before_gap..]
            .iter()
            .any(|line| line.starts_with("tooltip_text ")),
        "blank space after a herd count has no tooltip"
    );
}

#[test]
fn pop_card_tooltip_dispatch_opens_its_food_breakdown() {
    let mut rig = rig(&[
        r#"time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("pops", true)
local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
local P = require("lib.ui").P
local left, top = card_object.origin_x + card_object.x, card_object.origin_y + card_object.y + card_object.tops[1]
note(string.format("pop_food_point %g %g", left + 45 * P, top + 31 * P))"#,
    ]);
    rig.opened();
    let pop = point_note(&rig, "pop_food_point ");
    assert!(
        rig.key_at(pop.0, pop.1).is_some(),
        "the pop card has a passive hit key"
    );
    rig.feed(&[PointerEvent::Move(points(pop.0, pop.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line == "tooltip_text oats"),
        "the pop card's food bar opens its shared per-good tooltip: {:?}",
        notes(&rig)
    );
}

#[test]
fn pop_card_fields_route_to_their_shared_tooltips() {
    let mut rig = rig(&[
        r#"time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("pops", true)
local function text(content)
  if content == nil then return "nil" end
  if content.title ~= nil then return "title=" .. content.title end
  local lines = {}
  for _, line in ipairs(content) do
    local chunks = {}
    if type(line) == "table" then
      for _, span in ipairs(line) do chunks[#chunks + 1] = type(span) == "table" and span.text or tostring(span) end
    else chunks[1] = tostring(line) end
    lines[#lines + 1] = table.concat(chunks)
  end
  if #lines == 13 then return lines[1] .. "|... 13 lines" end
  return table.concat(lines, "|")
end
local function tip(name, item, x, y)
  if type(item.tooltip) ~= "function" then note(name .. " <missing>"); return end
  local content, part = item.tooltip(x, y)
  note(name .. " " .. text(content) .. " part=" .. tostring(part or "-"))
end
local P = require("lib.ui").P
local pop = card_object.items[1]
note_tooltip_kind("pop_card_item", card_object.base .. "_item_" .. card_object.generation .. "_1")
local width = card_object.width // P
tip("pop_kind", pop, 20, 4)
tip("pop_culture", pop, 20, 15)
tip("pop_religion", pop, 17 + font.layout("ulaid · ").width + 1, 15)
tip("pop_size", pop, width - 12, 4)
tip("pop_trend", pop, width - 35, 15)
tip("pop_food_word", pop, 8, 26)
tip("pop_food_bar", pop, 45, 31)
tip("pop_food_days", pop, 80, 26)
tip("pop_job_word", pop, 110, 26)
tip("pop_job_bar", pop, 140, 31)
tip("pop_needs_word", pop, 12, 38)
tip("pop_needs_bar", pop, 40, 43)
tip("pop_mood_word", pop, 80, 38)
tip("pop_mood_value", pop, 110, 38)
tip("pop_housing_word", pop, 15, 49)
tip("pop_housing_value", pop, 60, 49)
tip("pop_crowd_word", pop, 112, 49)
tip("pop_crowd_value", pop, 150, 49)

"#,
    ]);
    rig.opened();
    let captured = notes(&rig);
    for expected in [
        "pop_kind title=peasant part=kind",
        "pop_culture title=Ulaid part=culture",
        "pop_religion title=Christianity part=religion",
        "pop_size workers 108|dependents 72|births 2, deaths 1 part=size",
        "pop_trend workers 108|dependents 72|births 2, deaths 1 part=trend",
        "pop_food_word title=food days part=food",
        "pop_food_bar oats 300 bushels, 65% of meals|barley 100 bushels, 34% of meals part=food_bar",
        "pop_food_days oats 300 bushels, 65% of meals|barley 100 bushels, 34% of meals part=food_days",
        "pop_job_word title=jobs part=job",
        "pop_job_bar employed 96|unemployed 12 part=job_bar",
        "pop_needs_word title=needs part=needs",
        "pop_needs_bar title=needs part=needs_bar",
        "pop_mood_word title=satisfaction part=mood",
        "pop_mood_value title=satisfaction part=mood_value",
        "pop_housing_word title=housing part=housing",
        "pop_housing_value basic 380|makeshift 32 part=housing_value",
        "pop_crowd_word title=crowding part=crowd",
        "pop_crowd_value crowding 95%|housing 412 part=crowd_value",
    ] {
        assert!(
            captured.iter().any(|line| line == expected),
            "missing {expected}: {captured:?}"
        );
    }
    assert!(
        captured
            .iter()
            .any(|line| line == "tooltip_kind pop_card_item lockable"),
        "pop card fields remain lockable: {captured:?}"
    );
}

#[test]
fn building_card_fields_route_to_their_shared_tooltips() {
    let mut rig = rig(&[r#"VIEWS[1].cards.buildings[1].herds = {
  {species = "cattle", head = 27, maximum = 36},
  {species = "pigs", head = 9, maximum = 12},
  {species = "sheep", head = 10, maximum = 20},
}

time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("buildings", true)
local function text(content)
  if content == nil then return "nil" end
  if content.title ~= nil then return "title=" .. content.title end
  local lines = {}
  for _, line in ipairs(content) do
    local chunks = {}
    if type(line) == "table" then
      for _, span in ipairs(line) do chunks[#chunks + 1] = type(span) == "table" and span.text or tostring(span) end
    else chunks[1] = tostring(line) end
    lines[#lines + 1] = table.concat(chunks)
  end
  if #lines == 13 then return lines[1] .. "|... 13 lines" end
  return table.concat(lines, "|")
end
local function tip(name, item, x, y)
  if type(item.tooltip) ~= "function" then note(name .. " <missing>"); return end
  local content, part = item.tooltip(x, y)
  note(name .. " " .. text(content) .. " part=" .. tostring(part or "-"))
end
local P = require("lib.ui").P
local building = card_object.items[1]
note_tooltip_kind("building_card_item", card_object.base .. "_item_" .. card_object.generation .. "_1")
local width = card_object.width // P
local positions_width, storage_width = font.layout("positions").width, font.layout("storage").width
local count = "80/88"
local count_width = font.layout(count).width
local fixed = 5 + positions_width + 4 + 4 + count_width + 12 + storage_width + 4 + 6
local room = width - fixed
local positions_bar_width = room >= 34 and 24 or math.floor(room / 2)
local positions_bar_x = 5 + positions_width + 4
local count_x = positions_bar_x + positions_bar_width + 4
local storage_x = count_x + count_width + 12
local storage_bar_x = storage_x + storage_width + 4
tip("building_kind", building, 20, 4)
local wage_text = "wage 3.2"
local wage_left = width - 5 - font.layout(wage_text).width
tip("building_wage_word", building, wage_left + 1, 4)
tip("building_wage_value", building, wage_left + font.layout("wage ").width + 1, 4)
tip("building_method_one", building, 22, 15)
tip("building_method_two", building, 110, 15)
tip("building_housing", building, width - 28, 15)
tip("building_positions_word", building, 10, 26)
tip("building_positions_bar", building, positions_bar_x + positions_bar_width // 2, 31)
tip("building_positions_count", building, count_x + 1, 26)
tip("building_storage_word", building, storage_x + 1, 26)
tip("building_storage_bar", building, storage_bar_x + 1, 31)
tip("building_herd_icon", building, 9, 42)
tip("building_herd_count", building, 20, 42)
local pigs_x = 5 + 11 + font.layout("27/36").width + 10
tip("building_pigs_icon", building, pigs_x + 2, 42)
tip("building_pigs_count", building, pigs_x + 12, 42)"#]);
    rig.opened();
    let captured = notes(&rig);
    for expected in [
        "building_kind title=farm part=kind",
        "building_wage_word title=wage part=wage_word",
        "building_wage_value May 769: 3.2 per head (18 paid)|... 13 lines part=wage",
        "building_method_one title=mixed farming part=mixed_farming",
        "building_method_two title=coastal subsistence part=coastal_subsistence",
        "building_housing title=housing part=housing",
        "building_positions_word title=position part=positions",
        "building_positions_bar filled 80|open 8 part=staff_bar",
        "building_positions_count filled 80|open 8 part=staff_count",
        "building_storage_word title=storage part=storage",
        "building_storage_bar used 6400 L of 8000 L part=store_bar",
        "building_herd_icon title=cattle part=cattle",
        "building_herd_count cattle 27 of 36 part=cattle",
        "building_pigs_icon title=pigs part=pigs",
        "building_pigs_count pigs 9 of 12 part=pigs",
    ] {
        assert!(
            captured.iter().any(|line| line == expected),
            "missing {expected}: {captured:?}"
        );
    }
    assert!(
        captured
            .iter()
            .any(|line| line == "tooltip_kind building_card_item lockable"),
        "building card fields remain lockable: {captured:?}"
    );
}

#[test]
fn building_card_hover_restarts_when_moving_from_term_to_its_bar() {
    let mut rig = rig(&[
        r#"time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("buildings", true)
local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
local P = require("lib.ui").P
local item = card_object.items[1]
local positions_width, storage_width = font.layout("positions").width, font.layout("storage").width
local building = VIEWS[1].cards.buildings[1]
local count_width = font.layout(building.filled .. "/" .. (building.filled + building.open)).width
local W = card_object.width // P
local R = W - (5 + positions_width + 4 + 4 + count_width + 12 + storage_width + 4 + 6)
local positions_bar_width = R >= 34 and 24 or math.floor(R / 2)
local left, top = card_object.origin_x + card_object.x, card_object.origin_y + card_object.y + card_object.tops[1]
note(string.format("positions_point %g %g", left + (5 + positions_width // 2) * P, top + 26 * P))
note(string.format("positions_bar_point %g %g", left + (5 + positions_width + 4 + positions_bar_width // 2) * P, top + 31 * P))"#,
    ]);
    rig.opened();

    let positions = point_note(&rig, "positions_point ");
    let positions_bar = point_note(&rig, "positions_bar_point ");
    assert!(rig.key_at(positions.0, positions.1).is_some());
    assert!(rig.key_at(positions_bar.0, positions_bar.1).is_some());
    rig.feed(&[PointerEvent::Move(points(positions.0, positions.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip_text position"),
        "the positions explanation appears after its hover delay: {:?}",
        notes(&rig)
    );

    let before_bar = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(positions_bar.0, positions_bar.1))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert!(
        !notes(&rig)[before_bar..]
            .iter()
            .any(|line| line == "tooltip_text 80"),
        "moving from the term to its class bar restarts the tooltip delay"
    );
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig)[before_bar..]
            .iter()
            .any(|line| line == "tooltip_text 80"),
        "the staff class appears after the restarted delay: {:?}",
        notes(&rig)
    );
}

#[test]
fn a_changed_tooltip_delay_is_used_by_the_next_shipped_tooltip() {
    let directory = crate::testing::Dir::new();
    let config_path = directory.0.join("config.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let config = crate::config::load(&config_path).expect("load isolated config");
    let settings = crate::settings::Runtime::new(&config, None).expect("settings runtime");
    let mut rig = rig_with_settings(
        &[
            r#"local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
settings.set("ui.tooltips.delay", 1.5); time(1, 1); announce_main_tooltips()"#,
        ],
        settings.extension(),
    );
    rig.opened();

    let recorded = notes(&rig);
    let hit = recorded
        .iter()
        .find_map(|line| line.strip_prefix("date_hit "))
        .expect("the shipped UI publishes the date tooltip hit region");
    let rect = hit
        .split_whitespace()
        .map(|value| value.parse::<f64>().expect("numeric date hit bound"))
        .collect::<Vec<_>>();
    let point = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    rig.feed(&[PointerEvent::Move(points(point.0, point.1))]);
    for _ in 0..24 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        !notes(&rig).iter().any(|line| line == "tooltip_text date"),
        "the changed delay has not elapsed after 24 frame updates"
    );
    for _ in 0..100 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line == "tooltip_text date"),
        "the shipped tooltip appears after the delay measured by frame updates"
    );
}

#[test]
fn a_changed_disabled_tooltip_lock_never_locks_the_next_shipped_tooltip() {
    let directory = crate::testing::Dir::new();
    let config_path = directory.0.join("config.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let config = crate::config::load(&config_path).expect("load isolated config");
    let settings = crate::settings::Runtime::new(&config, None).expect("settings runtime");
    let mut rig = rig_with_settings(
        &[
            r#"local text = require("lib.text"); local real_set = text.set
        local real_destroy = text.destroy
text.set = function(line, value)
  line.ck_date = value == "date"
  real_set(line, value)
  if capture_tooltips then note("tooltip_text " .. value) end
end
text.destroy = function(line)
  if line.ck_date then note("tooltip_destroy date") end
  return real_destroy(line)
end
capture_tooltips = true
settings.set("ui.tooltips.lock", "disabled"); time(1, 1); announce_main_tooltips()"#,
        ],
        settings.extension(),
    );
    rig.opened();
    rig.feed(&[PointerEvent::Move(main_date_pointer(&rig))]);
    for _ in 0..100 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(notes(&rig).iter().any(|line| line == "tooltip_text date"));
    rig.feed(&[PointerEvent::Move(points(1700.0, 800.0))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip_destroy date"),
        "a tooltip with locking disabled is destroyed when its hover ends"
    );
}

#[test]
fn a_changed_tooltip_lock_timer_locks_the_next_shipped_tooltip_at_the_new_time() {
    let directory = crate::testing::Dir::new();
    let config_path = directory.0.join("config.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let config = crate::config::load(&config_path).expect("load isolated config");
    let settings = crate::settings::Runtime::new(&config, None).expect("settings runtime");
    let mut rig = rig_with_settings(
        &[
            r#"local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
settings.set("ui.tooltips.lock_timer", 0.3); time(1, 1); announce_main_tooltips()"#,
        ],
        settings.extension(),
    );
    rig.opened();
    rig.feed(&[PointerEvent::Move(main_date_pointer(&rig))]);
    for _ in 0..70 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(notes(&rig).iter().any(|line| line == "tooltip_text date"));
    assert_eq!(tooltip_panel_count(&rig), 1, "the date tooltip opened");

    rig.feed(&[PointerEvent::Move(points(1700.0, 800.0))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert_eq!(
        tooltip_panel_count(&rig),
        1,
        "the tooltip stays open after the changed lock timer expires"
    );
}

fn main_date_pointer(rig: &Rig) -> PointerPosition {
    let hit = notes(rig)
        .into_iter()
        .find_map(|line| line.strip_prefix("date_hit ").map(str::to_owned))
        .expect("the shipped UI publishes the date tooltip hit region");
    let rect = hit
        .split_whitespace()
        .map(|value| value.parse::<f64>().expect("numeric date hit bound"))
        .collect::<Vec<_>>();
    points(rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)
}

fn tooltip_panel_count(rig: &Rig) -> usize {
    rig.harness
        .bridge
        .hit_regions()
        .expect("published hit regions")
        .regions()
        .iter()
        .filter(|region| region.key().starts_with("tooltip_panel_"))
        .count()
}

#[test]
fn pop_and_building_rows_route_summary_and_open_details_to_the_same_tooltips() {
    let mut rig = rig(&[
        r#"VIEWS[1].cards.crowding_pm = 1120
VIEWS[1].cards.pops[1].crowding_pm = 1120
VIEWS[1].cards.buildings[1].store_used_l, VIEWS[1].cards.buildings[1].store_capacity_l = 60010, 60000
colour_watch = true; time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("pops", true)
"#,
        r#"look_buttons.rows.on_click()
local function text(content)
  if content == nil then return "nil" end
  if content.title ~= nil then return "title=" .. content.title end
  local lines = {}
  for _, line in ipairs(content) do
    local chunks = {}
    if type(line) == "table" then
      for _, span in ipairs(line) do chunks[#chunks + 1] = type(span) == "table" and span.text or tostring(span) end
    else chunks[1] = tostring(line) end
    lines[#lines + 1] = table.concat(chunks)
  end
  if #lines == 13 then return lines[1] .. "|... 13 lines" end
  return table.concat(lines, "|")
end
local function tip(name, item, x, y)
  if type(item.tooltip) ~= "function" then note(name .. " <missing>"); return end
  local content, part = item.tooltip(x, y)
  note(name .. " " .. text(content) .. " part=" .. tostring(part or "-"))
end
tip("pop_row_kind", card_object.items[2], 20, 2)
tip("pop_row_size", card_object.items[2], card_object.width // 2 - 75, 2)
card_object.items[2].on_click()
tip("pop_row_open_kind", card_object.items[2], 20, 2)
tip("pop_row_open_size", card_object.items[2], card_object.width // 2 - 75, 2)
tip("pop_row_open_food", card_object.items[2], card_object.width // 2 - 33, 2)
tip("pop_row_open_mood", card_object.items[2], card_object.width // 2 - 4, 2)
tip("pop_row_open_culture", card_object.items[2], 18, 14)
tip("pop_row_open_religion", card_object.items[2], 16 + font.layout("Ulaid · ").width + 1, 14)
tip("pop_row_detail_size", card_object.items[2], 30, 27)
tip("pop_row_detail_jobs", card_object.items[2], 30, 39)
tip("pop_row_detail_housing", card_object.items[2], 30, 75)
tip("pop_row_detail_crowding", card_object.items[2], 30, 87)

tabs_by_title.Cards:select("buildings", true)
local building = card_object.items[2]
tip("building_row_kind", building, 20, 2)
tip("building_row_staff", building, card_object.width // 2 - 55, 2)
tip("building_row_wage", building, card_object.width // 2 - 8, 2)
building.on_click()
building = card_object.items[2]
tip("building_row_method_origin", building, 16, 13)
tip("building_row_method", building, 45, 15)
tip("building_row_store_label", building, 20, 27)
tip("building_row_store_value", building, 90, 27)
tip("building_row_herd_icon", building, 20, 39)
tip("building_row_herd_count", building, 45, 39)
tip("building_row_positions_label", building, 20, 75)
tip("building_row_positions_value", building, 90, 75)
tip("building_row_housing_detail", building, 40, 87)"#,
    ]);
    rig.opened();
    rig.opened();
    rig.deliver_fixture_frame();
    let captured = notes(&rig);
    for expected in [
        "pop_row_kind title=peasant part=kind",
        "pop_row_size workers 108|dependents 72|births 2, deaths 1 part=size",
        "pop_row_open_kind title=peasant part=kind",
        "pop_row_open_size workers 108|dependents 72|births 2, deaths 1 part=size",
        "pop_row_open_food oats 300 bushels, 65% of meals|barley 100 bushels, 34% of meals part=food_days",
        "pop_row_open_mood title=satisfaction part=mood",
        "pop_row_open_culture title=Ulaid part=culture",
        "pop_row_open_religion title=Christianity part=religion",
        "pop_row_detail_size workers 108|dependents 72|births 2, deaths 1 part=size",
        "pop_row_detail_jobs employed 96|unemployed 12 part=jobs",
        "pop_row_detail_housing basic 380|makeshift 32 part=pop_housing",
        "pop_row_detail_crowding crowding 112%|housing 412 part=pop_crowding",
        "building_row_kind title=farm part=kind",
        "building_row_staff filled 80|open 8 part=staff",
        "building_row_wage May 769: 3.2 per head (18 paid)|... 13 lines part=wage",
        "building_row_method_origin title=mixed farming part=mixed_farming",
        "building_row_method title=mixed farming part=mixed_farming",
        "building_row_store_label title=storage part=storage",
        "building_row_store_value used 60010 L of 60000 L part=store",
        "building_row_herd_icon title=cattle part=cattle",
        "building_row_herd_count cattle 27 of 36 part=cattle",
        "building_row_positions_label title=position part=positions",
        "building_row_positions_value filled 80|open 8 part=staff",
        "building_row_housing_detail title=housing part=housing",
    ] {
        assert!(
            captured.iter().any(|line| line == expected),
            "missing {expected}: {:?}",
            captured
                .iter()
                .filter(|line| line.contains("_row_"))
                .collect::<Vec<_>>()
        );
    }
    assert_pop_and_store_limit_tints(&captured);
}

fn assert_pop_and_store_limit_tints(captured: &[String]) {
    assert!(
        tint_line(captured, "112%", "#f66155", "112%"),
        "the over-limit crowding number is red in the pop card or open row: {captured:?}"
    );
    assert!(
        tint_line(captured, "storage 60010 / 60000 L", "#f66155", "60010"),
        "the over-limit store amount is red in the open building row: {captured:?}"
    );
    assert!(
        tint_line(captured, "storage 60010 / 60000 L", "#5fb35a", "60000"),
        "the store capacity remains green: {captured:?}"
    );
}

#[test]
fn open_building_row_tooltip_dispatch_tracks_the_herd_icon() {
    let mut rig = rig(&[r#"VIEWS[1].cards.buildings[1].herds = {
  {species = "cattle", head = 27, maximum = 36},
  {species = "pigs", head = 9, maximum = 12},
  {species = "sheep", head = 10, maximum = 20},
}
time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("buildings", true)
local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
look_buttons.rows.on_click(); card_object.items[2].on_click()
local content, part = card_object.items[2].tooltip(20, 54)
note("row_pigs_source title=" .. content.title .. " part=" .. tostring(part))
local above = card_object.items[2].tooltip(30, 48)
note("row_pigs_above_count " .. (above == nil and "nil" or "present"))
local count, count_part = card_object.items[2].tooltip(30, 49)
local count_lines = {}
for _, line in ipairs(count) do
  if type(line) == "table" then
    local chunks = {}
    for _, span in ipairs(line) do chunks[#chunks + 1] = type(span) == "table" and span.text or tostring(span) end
    count_lines[#count_lines + 1] = table.concat(chunks)
  else count_lines[#count_lines + 1] = tostring(line) end
end
note("row_pigs_count " .. table.concat(count_lines, "|") .. " part=" .. tostring(count_part))
local P = require("lib.ui").P
local left = card_object.origin_x + card_object.x
local top = card_object.origin_y + card_object.y + card_object.tops[2]
note(string.format("row_pigs_point %g %g", left + 20 * P, top + 54 * P))"#]);
    rig.opened();
    let pigs = point_note(&rig, "row_pigs_point ");
    assert!(
        rig.key_at(pigs.0, pigs.1).is_some(),
        "the open herd icon is inside its row hit key"
    );
    rig.feed(&[PointerEvent::Move(points(pigs.0, pigs.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line == "tooltip_text pigs"),
        "real row pointer dispatch should open the pigs tooltip at the herd line: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_pigs_source title=pigs part=pigs"),
        "the open-row provider returns pigs with the pigs part"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_pigs_count pigs 9 of 12 part=pigs"),
        "the second herd count uses the pigs breakdown, not the first herd: {:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.contains("row_pigs_"))
            .collect::<Vec<_>>()
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_pigs_above_count nil"),
        "the herd count hit region starts at the drawn text, not one pixel above it"
    );
}

#[test]
fn the_date_and_yield_map_mode_use_their_glossary_explanations() {
    let mut rig = rig(&["time(1, 1) announce_main_tooltips()"]);
    rig.opened();
    let captured = notes(&rig);
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip date title=date|")),
        "{captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip yield title=yield map|")),
        "{captured:?}"
    );
    let hit = captured
        .iter()
        .find_map(|line| line.strip_prefix("date_hit "))
        .expect("main.lua publishes the date readout hit region");
    let rect = hit
        .split_whitespace()
        .map(|value| value.parse::<f64>().expect("numeric date hit bound"))
        .collect::<Vec<_>>();
    assert_eq!(rect.len(), 4, "{hit}");
    let date_point = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    assert_eq!(
        rig.key_at(date_point.0, date_point.1).as_deref(),
        Some("date_readout"),
        "the date's own hit rectangle routes to its explanation key"
    );
}

#[test]
fn location_tab_words_numbers_and_window_title_open_their_sources() {
    let mut rig = rig(&["time(1, 1) select_place('rowan', 'Rowan'); announce_location_tooltips()"]);
    rig.opened();
    let captured = notes(&rig);
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip people_word title=population|")),
        "{captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line
                .starts_with("tooltip people_number peasants 120|workers 80, dependents 40")),
        "{captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip food_number peasant Ulaid 215 days")),
        "{captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip title title=Rowan|")),
        "{captured:?}"
    );
    for name in ["title", "people_word", "people_number", "food_number"] {
        assert!(
            captured
                .iter()
                .any(|line| line == &format!("tooltip_kind {name} lockable")),
            "{name} stays lockable: {captured:?}"
        );
    }
    let mut tab_kinds = captured
        .iter()
        .filter(|line| line.starts_with("tooltip_kind tab_"));
    assert!(
        tab_kinds.clone().count() > 0,
        "tab kinds were captured: {captured:?}"
    );
    assert!(
        tab_kinds.all(|line| line.ends_with(" lockable")),
        "all location tab cells stay lockable: {captured:?}"
    );
    let expected = [
        ("tab_1_1", "title=population|"),
        ("tab_1_2", "peasants 120|"),
        ("tab_2_1", "title=worker|"),
        ("tab_2_2", "peasants 120|"),
        ("tab_3_1", "title=dependent|"),
        ("tab_3_2", "peasants 120|"),
        ("tab_4_1", "title=housing|"),
        ("tab_4_2", "title=basic housing|"),
        ("tab_4_3", "basic "),
        ("tab_4_5", "title=makeshift housing|"),
        ("tab_4_6", "basic "),
        ("tab_5_1", "title=crowding|"),
        ("tab_5_2", "people "),
        ("tab_6_1", "title=food days|"),
        ("tab_6_2", "peasant "),
        ("tab_7_1", "title=building land|"),
        ("tab_7_2", "in use "),
        ("tab_8_1", "title=storage|"),
        ("tab_8_2", "pops "),
        ("tab_9_1", "title=overflow|"),
        ("tab_9_2", "nothing lost"),
    ];
    for (name, expected) in expected {
        assert!(
            captured
                .iter()
                .any(|line| line.starts_with(&format!("tooltip {name} {expected}"))),
            "{name} must route to {expected}: {captured:?}"
        );
    }
}

#[test]
fn a_wrapped_housing_continuation_keeps_the_place_class_source() {
    let mut rig = rig(&[
        "colour_watch = true; time(1, 1); select_place('rowan', 'Rowan'); announce_wrapped_housing_source()",
        "time(1, 1)",
    ]);
    rig.opened();
    rig.deliver_fixture_frame_at(1278, 900, 2);
    rig.opened();
    let housing_rows = notes(&rig)
        .iter()
        .filter_map(|line| line.strip_prefix("housing_row ").map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(
        housing_rows,
        vec!["  80/100,", "  makeshift 10/20"],
        "wrapped housing text matches the old wrap() layout at the same width"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("housing_continuation ")),
        "a continued housing number retains the housing class source: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "housing_continuation_found true"),
        "the actual place line wraps at this test width: {:?}",
        notes(&rig)
    );
    let recorded = notes(&rig);
    assert!(
        tint_line(&recorded, "makeshift ", "#92ddea", "makeshift "),
        "wrapped term retains term tint: {recorded:?}"
    );
    assert!(
        tint_line(&recorded, "10/20", "#5fb35a", "10"),
        "wrapped first number retains number tint: {recorded:?}"
    );
    assert!(
        tint_line(&recorded, "10/20", "#ece3cb", "/"),
        "wrapped slash retains text tint: {recorded:?}"
    );
    assert!(
        tint_line(&recorded, "10/20", "#5fb35a", "20"),
        "wrapped second number retains number tint: {recorded:?}"
    );
}

#[test]
fn ledger_pop_row_explains_its_kind_and_uses_its_size_and_place_cells() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_ledger_tabs()",
        "announce_ledger_pop_tips()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    let captured = notes(&rig);
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip pop_icon title=")),
        "the row's icon explains the pop kind: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line == "tooltip_kind pop_icon lockable"),
        "ledger pop rows remain lockable: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip pop_kind title=")),
        "the kind and culture cell explains the pop kind: {captured:?}"
    );
    let size = captured
        .iter()
        .find(|line| line.starts_with("tooltip pop_size workers "))
        .expect("the size cell opens the selected pop's workers and dependents");
    let numbers = |line: &str| -> Vec<u32> {
        line.split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect()
    };
    let size_numbers = numbers(size);
    let total = captured
        .iter()
        .find(|line| line.starts_with("pop_size_total "))
        .expect("the displayed pop size");
    let shown = numbers(total);
    assert!(
        size.contains("dependents ") && size.contains("births ") && size.contains("deaths "),
        "{size}"
    );
    assert_eq!(
        size_numbers[0] + size_numbers[1],
        shown[0],
        "the breakdown is this row's size: {size} {total}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip pop_place title=")),
        "the place cell explains the place: {captured:?}"
    );
}
