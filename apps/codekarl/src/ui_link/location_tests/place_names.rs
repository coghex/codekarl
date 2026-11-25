//! The place-name choice (CK-423) in the shipped UI: a change of the choice relabels the views already open, by place id, while the
//! game is paused and without a new revision (no tick).
use super::characters::{CHARACTERS, card_point, click, note_line};
use super::*;
use crate::ui_link::widget_witness_tests::points;
use ck_window::PointerPosition;

/// Observers set before `main.lua` runs: every text a tooltip sets is noted (once `capture_tooltips` is on), and the event log the shipped
/// script makes is kept.
const PRELUDE: &str = r#"
local text = require("lib.text")
local real_set = text.set
text.set = function(line, value)
  real_set(line, value)
  if capture_tooltips then note("tooltip_text " .. tostring(value)) end
end
local event_log = require("lib.event_log")
local real_log_new = event_log.new
event_log.new = function(spec) event_log_object = real_log_new(spec); return event_log_object end
"#;

/// A `game.places` that applies the choice the way the host does, an event log with one blight row naming that place, and the first
/// `game_time` of a paused game. `time_deliveries` counts the `game_time` callbacks: the test changes the choice without another.
const SETUP: &str = r#"
choice = "english"
place_data = {{id = "rowan", local_name = "Ruadhán", english_name = "Rowan", kind = "town", size = 3}}
game.places = function()
  local out = {}
  for index, place in ipairs(place_data) do
    local copy = {}
    for key, value in pairs(place) do copy[key] = value end
    copy.name = choice == "local" and place.local_name or place.english_name or place.local_name
    out[index] = copy
  end
  return out
end
game.events = function(_)
  return {{kind = "blight", action = "begins", good = "oats", place = game.places()[1].name, place_id = "rowan",
    year = 768, month = 5, day = 2, hour = 6}}
end
local real_time = callbacks.time
time_deliveries = 0
callbacks.time = function(snapshot) time_deliveries = time_deliveries + 1; return real_time(snapshot) end
callbacks.time({year = 768, month = 1, day = 1, hour = 1, paused = true, speed = 1, economy_revision = 1,
  events_revision = 1, world = 0, log_storms = true, log_threshold = 34, log_blights = true, log_sort = "time", log_descending = true})
select_place("rowan", game.places()[1].name)
event_log_object:open()
note("panel_before " .. tabs_object.buttons[1].line.ck_test_text)
note("log_before " .. event_log_object.rows[1].place)
capture_tooltips = true
callbacks.hover({id = "rowan", name = game.places()[1].name, left = 880, right = 900, y = 440})
"#;

const CHANGE: &str = r#"
choice = "local"
require("lib.place_names").set("local")
note("panel_after " .. tabs_object.buttons[1].line.ck_test_text)
note("log_after " .. event_log_object.rows[1].place)
note("deliveries " .. time_deliveries)
"#;

#[test]
fn a_changed_choice_relabels_an_open_panel_tooltip_and_event_log_while_paused() {
    // The tooltip system reads the tooltip settings, so the rig has a settings runtime, as the tooltip-lock tests do.
    let directory = crate::testing::Dir::new();
    let config_path = directory.0.join("config.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let config = crate::config::load(&config_path).expect("load isolated config");
    let settings = crate::settings::Runtime::new(&config, None).expect("settings runtime");
    let mut rig = Rig::new_fixture_body_with_settings(
        &script_with_prelude(PRELUDE, &[SETUP, CHANGE]),
        settings.extension(),
    );
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    for _ in 0..100 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    let before = notes(&rig);
    let panels = rig
        .harness
        .bridge
        .hit_regions()
        .expect("published hit regions")
        .regions()
        .iter()
        .filter(|region| region.key().starts_with("tooltip_panel_"))
        .count();
    assert_eq!(panels, 1, "the map tooltip is open");
    assert!(
        before.iter().any(|line| line == "panel_before Rowan"),
        "{before:?}"
    );
    assert!(
        before.iter().any(|line| line == "log_before Rowan"),
        "{before:?}"
    );
    assert!(
        before.iter().any(|line| line == "tooltip_text Rowan"),
        "the map tooltip opened with the English name: {before:?}"
    );
    assert!(
        !before.iter().any(|line| line == "tooltip_text Ruadhán"),
        "{before:?}"
    );
    rig.opened();
    let after = notes(&rig);
    assert!(
        after.iter().any(|line| line == "panel_after Ruadhán"),
        "the open panel's first tab: {after:?}"
    );
    assert!(
        after.iter().any(|line| line == "log_after Ruadhán"),
        "the open event log's row: {after:?}"
    );
    assert!(
        after.iter().any(|line| line == "tooltip_text Ruadhán"),
        "the open map tooltip: {after:?}"
    );
    assert!(
        after.iter().any(|line| line == "deliveries 1"),
        "no game_time was delivered, so no tick or new revision: {after:?}"
    );
}

#[test]
fn the_place_names_module_keeps_the_choice_and_tells_every_listener_once_per_change() {
    let mut rig = rig(&[r#"
local names = require("lib.place_names")
assert(names.get() == "english", "the default")
local seen = {}
local off = names.on_change(function(value) seen[#seen + 1] = value end)
assert(names.set("local") and names.get() == "local")
assert(not names.set("local"), "the same choice is no change")
assert(names.set("english"))
assert(table.concat(seen, ",") == "local,english")
off()
names.set("local")
assert(#seen == 2, "a removed listener hears nothing")
assert(not pcall(names.set, "french"), "only english and local")
"#]);
    rig.opened();
}

/// Locked tooltips and the place-name choice (reviews 1 and 2 of CK-423). The game stays paused: `ptime()` delivers a paused `game_time` of
/// the same hour and revision, which is what the shipped UI gets between presses. The choice is changed by routing a real pointer press and
/// release to the General tab's "local" button, and the stubs read the choice from the settings runtime, as the host does.
const LOCKED: &str = r#"
settings.set("ui.tooltips.lock_timer", 0.3)
function chosen() return settings.graphics().place_names end
replaced = false
-- Niall's card names his place and sorts last under local, as a list ordered by a name would change.
game.characters = function(_)
  local list = {}
  for index, c in ipairs(CH[1]) do
    local copy = {}
    for key, value in pairs(c) do copy[key] = value end
    if c.id == 1 then
      copy.role = chosen() == "local" and "ruler of Biorra" or "ruler of Birr"
      copy.importance = chosen() == "local" and 9 or 0
      -- A loaded game: another person holds the number.
      if replaced then copy.name, copy.epithet, copy.role = "Different character", "", "ruler of Cork" end
    end
    list[index] = copy
  end
  return list
end
-- Two places whose order follows the shown name: rowan is Alder (English) or Zulu (local), nobody is Beech or Birr.
local names = {rowan = {"Alder", "Zulu"}, nobody = {"Beech", "Birr"}}
game.places = function()
  local out = {}
  for _, id in ipairs(chosen() == "local" and {"nobody", "rowan"} or {"rowan", "nobody"}) do
    local pair = names[id]
    out[#out + 1] = {id = id, name = chosen() == "local" and pair[2] or pair[1], kind = "town", size = 2}
  end
  return out
end
local real_set = require("lib.text").set
require("lib.text").set = function(line, value)
  real_set(line, value)
  if capture_tooltips then note("tooltip_text " .. tostring(value)) end
end
capture_tooltips = true
function ptime()
  callbacks.time({year = 768, month = 1, day = 1, hour = 1, paused = true, speed = 1, economy_revision = 1, world = 0})
end
function open_general()
  local windows = require("lib.settings_window")
  windows.open({width = 1800, height = 900})
  local s = windows.current()
  assert(s.selected == "general", "the general tab")
  local b = s.controls.place_names[2]
  local ox, oy = s.window:content_origin()
  local w, h = b:size()
  note(string.format("local_button %g %g", ox + s.content_x + b.x + w / 2, oy + b.y + h / 2))
end
ptime()
announce_bars()
"#;

fn locked_rig(steps: &[&str]) -> Rig {
    let directory = crate::testing::Dir::new();
    let config_path = directory.0.join("config.toml");
    std::fs::write(&config_path, "").expect("write isolated config");
    let config = crate::config::load(&config_path).expect("load isolated config");
    let settings = crate::settings::Runtime::new(&config, None).expect("settings runtime");
    let mut all = vec![LOCKED];
    all.extend_from_slice(steps);
    let mut rig = Rig::new_fixture_body_with_settings(
        &script_with_prelude(CHARACTERS, &all),
        settings.extension(),
    );
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig
}

fn frames(rig: &mut Rig, count: usize) {
    for _ in 0..count {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
}

fn texts_after(rig: &Rig, marker: &str) -> Vec<String> {
    let all = notes(rig);
    let start = all
        .iter()
        .rposition(|line| line == marker)
        .unwrap_or_else(|| panic!("no note {marker}"));
    all[start..]
        .iter()
        .filter(|line| line.starts_with("tooltip_text"))
        .cloned()
        .collect()
}

/// Hovers (x, y) until its tooltip has opened and locked, then moves the pointer to an empty part of the map.
fn lock_tooltip_at(rig: &mut Rig, (x, y): (f64, f64)) {
    rig.feed(&[PointerEvent::Move(points(x, y))]);
    frames(rig, 70);
    rig.feed(&[PointerEvent::Move(PointerPosition {
        x: 300 * 256,
        y: 450 * 256,
    })]);
    frames(rig, 2);
}

fn press_local_button(rig: &mut Rig) {
    let line = note_line(rig, "local_button ");
    let mut values = line["local_button ".len()..].split_whitespace();
    let x: f64 = values.next().unwrap().parse().unwrap();
    let y: f64 = values.next().unwrap().parse().unwrap();
    click(rig, x, y);
}

#[test]
fn a_locked_card_tooltip_keeps_its_character_and_takes_the_new_name_through_the_routed_general_button()
 {
    let mut rig = locked_rig(&[
        "announce_ledger_origin()",
        "open_general() note('before_change')",
        "note('after_change ' .. chosen()) ptime() note('after_time')",
    ]);
    rig.opened();
    press_bar(&mut rig, "characters");
    rig.opened();
    let card = card_point(&rig, 1);
    lock_tooltip_at(&mut rig, card);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip_text ruler of Birr"),
        "the card's tooltip opened with the English role"
    );
    rig.opened();
    press_local_button(&mut rig);
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "after_change local"),
        "the routed press chose local"
    );
    let changed = texts_after(&rig, "before_change");
    assert!(
        changed
            .iter()
            .any(|line| line == "tooltip_text ruler of Biorra"),
        "the locked tooltip shows the new name: {changed:?}"
    );
    assert!(
        !changed
            .iter()
            .any(|line| line.contains("wife of the ruler")),
        "and is still Niall's, not the card that took his place in the list: {changed:?}"
    );
    // The next time delivery (paused, the same hour and revision; the step before) leaves the locked tooltip on its character.
    let later = texts_after(&rig, "after_change local");
    assert!(
        later
            .iter()
            .any(|line| line == "tooltip_text ruler of Biorra"),
        "{later:?}"
    );
    assert!(
        !later.iter().any(|line| line.contains("wife of the ruler")),
        "{later:?}"
    );
}

#[test]
fn a_locked_card_tooltip_takes_the_new_name_when_its_characters_tab_is_no_longer_showing() {
    let mut rig = locked_rig(&[
        "announce_ledger_origin()",
        "note('on_places') open_general() note('before_change')",
    ]);
    rig.opened();
    press_bar(&mut rig, "characters");
    rig.opened();
    let card = card_point(&rig, 1);
    lock_tooltip_at(&mut rig, card);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip_text ruler of Birr")
    );
    // The ledger shows the places now: the card stack that tooltip was opened over is gone.
    press_bar(&mut rig, "location");
    rig.opened();
    press_local_button(&mut rig);
    let changed = texts_after(&rig, "before_change");
    assert!(
        changed
            .iter()
            .any(|line| line == "tooltip_text ruler of Biorra"),
        "the locked tooltip of a hidden tab shows the new name: {changed:?}"
    );
}

#[test]
fn a_locked_places_row_tooltip_stays_on_its_place_after_the_rows_are_sorted_again_and_a_time_delivery()
 {
    let mut rig = locked_rig(&[
        "announce_ledger_origin()",
        "open_general() note('before_change')",
        "note('after_change ' .. chosen()) ptime() note('after_time')",
        "require('lib.settings_window').close() note('settings_closed')",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    // The first place row (the second row, under the header) is rowan, Alder under English.
    let (x, y) = {
        let line = note_line(&rig, "ledger_origin ");
        let mut values = line["ledger_origin ".len()..].split_whitespace();
        let x: f64 = values.next().unwrap().parse().unwrap();
        let y: f64 = values.next().unwrap().parse().unwrap();
        (x + 20.0, y + (2.0 - 0.5) * 13.0 * 2.0)
    };
    lock_tooltip_at(&mut rig, (x, y));
    assert!(
        notes(&rig).iter().any(|line| line == "tooltip_text Alder"),
        "the row's tooltip opened for rowan: {:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.starts_with("tooltip_text"))
            .collect::<Vec<_>>()
    );
    rig.opened();
    press_local_button(&mut rig);
    rig.opened();
    let changed = texts_after(&rig, "before_change");
    assert!(
        changed.iter().any(|line| line == "tooltip_text Zulu"),
        "rowan is Zulu now, and its locked tooltip says so: {changed:?}"
    );
    assert!(
        !changed.iter().any(|line| line == "tooltip_text Birr"),
        "not the place that took its row: {changed:?}"
    );
    let later = texts_after(&rig, "after_change local");
    assert!(
        later.iter().any(|line| line == "tooltip_text Zulu"),
        "the time delivery read the locked tooltip again, for its place: {later:?}"
    );
    assert!(
        !later.iter().any(|line| line == "tooltip_text Birr"),
        "a time delivery does not move the locked tooltip to another place: {later:?}"
    );
    // The pointer comes to the row that rowan left, which now belongs to the other place: that place's tooltip opens beside the locked one.
    rig.opened();
    rig.feed(&[PointerEvent::Move(points(x, y))]);
    frames(&mut rig, 70);
    let replacement = texts_after(&rig, "settings_closed");
    assert!(
        replacement.iter().any(|line| line == "tooltip_text Birr"),
        "the place now at that row gets its own tooltip: {replacement:?}"
    );
}

/// A load replaces the world and may give a character number to another person: a tooltip locked over the old character must not turn
/// into that person's (review 3 of CK-423), even when its card list is gone.
#[test]
fn a_locked_card_tooltip_does_not_outlive_a_world_a_load_replaced_whose_character_number_is_reused()
{
    let mut rig = locked_rig(&[
        "announce_ledger_origin()",
        "note('on_places')",
        "replaced = true callbacks.time({year = 768, month = 1, day = 1, hour = 1, paused = true, speed = 1, economy_revision = 2, world = 1}) note('world_replaced')",
    ]);
    rig.opened();
    press_bar(&mut rig, "characters");
    rig.opened();
    let card = card_point(&rig, 1);
    lock_tooltip_at(&mut rig, card);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "tooltip_text ruler of Birr")
    );
    press_bar(&mut rig, "location");
    rig.opened();
    frames(&mut rig, 3);
    let panels = |rig: &Rig| {
        rig.harness
            .bridge
            .hit_regions()
            .expect("published hit regions")
            .regions()
            .iter()
            .filter(|region| region.key().starts_with("tooltip_panel_"))
            .count()
    };
    assert_eq!(
        panels(&rig),
        1,
        "the locked tooltip is open before the load"
    );
    rig.opened();
    frames(&mut rig, 3);
    let after = texts_after(&rig, "world_replaced");
    assert!(
        !after
            .iter()
            .any(|line| line.contains("Different character") || line.contains("ruler of Cork")),
        "the replaced world's person never appears in the old tooltip: {after:?}"
    );
    assert_eq!(panels(&rig), 0, "the tooltip closed with its world");
}
