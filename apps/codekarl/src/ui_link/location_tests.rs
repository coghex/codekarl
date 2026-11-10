//! The location panel (milestone 9, slice 3) in a real UI VM: the shipped `scripts/ui/main.lua` run over a stub `game` whose
//! `game.place` serves fixture views, with the panel's list rows recorded as the lists are created and updated.
//!
//! A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use std::fmt::Write;

use ck_lua::ConfigValue;
use ck_window::PointerEvent;

use super::group_tests::Rig;
use super::widget_witness_tests::points;

/// The Lua before `main.lua`: the stub `game`, the fixture views (revision 1 and 2 of the place `rowan`, and an empty place) and
/// observers of the panel's lists, tabs and window.
const PREFIX: &str = r#"
callbacks = {}
place_calls = {}
local function price(good, milli, base) return {good = good, price_milli = milli, base = base} end
local function rowan(workers, births)
  return {
    id = "rowan", workers = workers, dependents = 40, food_days = 215,
    footprint_m2 = 4200, prepared_m2 = 24200, max_buildable_m2 = 25000,
    storage_used_l = 5400, storage_capacity_l = 86400, overflow_lost = 0,
    prices = {price("oats", 100000, 100), price("ale", 2500, 2)},
    pops = {{kind = "peasant", culture = "ulaid", religion = "christianity", workers = workers, dependents = 40, employed = 70,
      unemployed = workers - 70, food_days = 215, necessities = 0, niceties = 0, luxuries = 0, satisfaction = 0,
      births = births, deaths = 1}},
    buildings = {
      {kind = "farm", levels = 3, methods = {{id = "mixed_farming", levels = 3}}, filled = 25, open = 5,
        herds = {{species = "cattle", head = 27, maximum = 36}}, store_used_l = 1, store_capacity_l = 3500, wage = 0},
      {kind = "storehouse", levels = 1, methods = {}, filled = 0, open = 2, herds = {}, store_used_l = 0,
        store_capacity_l = 36000, wage = 0},
    },
  }
end
local nobody = {
  id = "nobody", workers = 0, dependents = 0, food_days = 0, footprint_m2 = 0, prepared_m2 = 0, max_buildable_m2 = 0, storage_used_l = 0, storage_capacity_l = 0, overflow_lost = 0,
  prices = {}, pops = {}, buildings = {},
}
-- A place whose pop's culture is a long word of fada-bearing letters, which wrapping must cut between characters.
local fada = rowan(80, 2)
fada.id = "fada"
fada.pops[1].culture = string.rep("\u{D3}", 40)
VIEWS = {[1] = {rowan = rowan(80, 2), nobody = nobody, fada = fada}, [2] = {rowan = rowan(90, 0), nobody = nobody}}
game = {
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  places = function() return {} end,
  place = function(id, revision)
    place_calls[#place_calls + 1] = id .. "@" .. revision
    local view = VIEWS[revision]
    if view == nil or view[id] == nil then error("game.place: no " .. id .. " at " .. revision) end
    return view[id]
  end,
}
-- The map-mode buttons (milestone 9, slice 8): `game.set_map_mode` records its calls, and fails when `fail_mode` is set.
set_mode_calls = {}
fail_mode = false
game.set_map_mode = function(name)
  if fail_mode then error("game.set_map_mode: refused") end
  set_mode_calls[#set_mode_calls + 1] = name
end
local buttons_library = require("lib.button")
local real_button_new = buttons_library.new
mode_buttons = {}
buttons_library.new = function(spec)
  local created = real_button_new(spec)
  if spec.text == "places" then places_button_object = created end
  if spec.icon == "stockpile" or spec.icon == "yield" or spec.icon == "elevation" then mode_buttons[spec.icon] = created end
  return created
end
function announce_modes()
  for _, key in ipairs({"stockpile", "yield", "elevation"}) do
    local button = mode_buttons[key]
    local width, height = button:size()
    note(string.format("mode %s %d %d %s", key, button.x + width / 2, button.y + height / 2, tostring(button.selected == true)))
  end
  note("modes_called " .. table.concat(set_mode_calls, ","))
end
-- Each mode button's and the places button's left edge, top edge, width and height, in window points.
function announce_bars()
  for _, key in ipairs({"stockpile", "yield", "elevation"}) do
    local button = mode_buttons[key]
    local width, height = button:size()
    note(string.format("bar %s %g %g %g %g", key, button.x, button.y, width, height))
  end
  local width, height = places_button_object:size()
  note(string.format("bar places %g %g %g %g", places_button_object.x, places_button_object.y, width, height))
end
function note(text) window.record(text) end
function time(hour, revision)
  callbacks.time({year = 768, month = 1, day = 1, hour = hour, paused = false, speed = 1, economy_revision = revision})
end
function select_place(id, name) callbacks.select({id = id, name = name, kind = "town", size = 3}) end

-- Every list's rows, as created and as replaced.
local lists = require("lib.list")
local real_list_new = lists.new
lists.new = function(spec)
  local created = real_list_new(spec)
  note("rows " .. table.concat(created.items, "|"))
  local real_set_items = created.set_items
  function created:set_items(items)
    note("rows " .. table.concat(items, "|"))
    real_set_items(self, items)
  end
  list_object = created
  return created
end
local tabs_library = require("lib.tabs")
local real_tabs_new = tabs_library.new
tabs_library.new = function(spec)
  tabs_object = real_tabs_new(spec)
  return tabs_object
end
local windows = require("lib.window")
local real_windows_new = windows.new
windows.new = function(spec)
  panel_window = real_windows_new(spec)
  return panel_window
end
-- The centre of each tab button, in window points, for the driver to click.
function announce_tabs()
  local x, y = panel_window:position()
  for index, item in ipairs(tabs_object.items) do
    local width, height = tabs_object.buttons[index]:size()
    note(string.format("tab %s %d %d %s", item.key, x + tabs_object.buttons[index].x + width / 2,
      y + tabs_object.buttons[index].y + height / 2, tabs_object.buttons[index].text or ""))
  end
end
-- Whether the title and the tabs sit inside the panel without touching: the shown title ends before the first tab, the last tab
-- ends inside the window, and the whole title is kept.
function announce_fit(expected_title)
  local width = panel_window:size()
  local first = tabs_object.buttons[1]
  local last = tabs_object.buttons[#tabs_object.buttons]
  local last_width = last:size()
  note(string.format("fit title=%s tabs_right=%s title_clear=%s", tostring(panel_window.title == expected_title),
    tostring(last.x + last_width <= width), tostring(8 * require("lib.ui").P + panel_window.title_text_width <= first.x)))
  note("tabs_left " .. tostring(first.x >= 0))
  local apart = true
  for index = 1, #tabs_object.buttons - 1 do
    local button, width = tabs_object.buttons[index], tabs_object.buttons[index]:size()
    apart = apart and button.x + width <= tabs_object.buttons[index + 1].x
  end
  apart = apart and last.x + last_width <= panel_window.outer_width and (panel_window.close_button == nil or last.x + last_width <= panel_window.close_button.x)
  note("apart " .. tostring(apart))
  note("shown " .. panel_window.shown_title)
  note("list_rows " .. list_object.rows)
end
function announce_floor() note("floor " .. require("lib.location").min_width()) end
function announce_geometry()
  local x, y = panel_window:position()
  local width, height = panel_window:size()
  note(string.format("panel %d %d %d %d", x, y, width, height))
end
steps = {}
"#;

fn script(steps: &[&str]) -> String {
    let main_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let main = std::fs::read_to_string(main_path).expect("the production UI script");
    let mut queued = String::new();
    for step in steps {
        writeln!(queued, "steps[#steps + 1] = function()\n{step}\nend")
            .expect("a string takes text");
    }
    // `main.lua` registers its own `on_opened`; the step runner replaces it, after it.
    format!(
        "{PREFIX}\n{main}\n{queued}\nwindow.on_opened(function() table.remove(steps, 1)() end)\n"
    )
}

/// A rig at 2 points per pixel, its window set to 1800 by 900 points: the left third is 600 points, wide enough for every fixture
/// line without wrapping.
fn rig(steps: &[&str]) -> Rig {
    let mut rig = Rig::new_fixture_body(&script(steps));
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig
}

fn notes(rig: &Rig) -> Vec<String> {
    rig.tables
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(|value| match value {
            ConfigValue::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The lists' rows, in the order they were created or replaced, as text.
fn rows(rig: &Rig) -> Vec<String> {
    notes(rig)
        .into_iter()
        .filter_map(|line| line.strip_prefix("rows ").map(str::to_owned))
        .collect()
}

fn tab_point(rig: &Rig, key: &str) -> (f64, f64) {
    let prefix = format!("tab {key} ");
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no tab {key}: {:?}", notes(rig)));
    let mut parts = line[prefix.len()..].split_whitespace();
    let x = parts.next().unwrap().parse().unwrap();
    let y = parts.next().unwrap().parse().unwrap();
    (x, y)
}

fn click_tab(rig: &mut Rig, key: &str) {
    click_tab_at(rig, key, (1800, 900));
}

/// Clicks the tab `key` and delivers a frame at the window size `(width, height)`.
fn click_tab_at(rig: &mut Rig, key: &str, (width, height): (u32, u32)) {
    let (x, y) = tab_point(rig, key);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    // A frame at the rig's own size, so that the pointer is dispatched without the window being resized.
    rig.deliver_fixture_frame_at(width, height, 2);
}

const LOCATION_ROWS: &str = "people: 120|  workers: 80|  dependents: 40|housing: basic 0/0, makeshift 0/0|crowding: 0%|food: 215 days|building land: 4200 / 24200 m²|storage: 5400 / 86400 L|lost to overflow last month: 0|prices (base):|  oats 100 (100)|  ale 2.5 (2)";
const POPS_ROWS: &str = "peasant ulaid christianity|  workers: 80|  dependents: 40|  housing: basic 0, makeshift 0|  crowding: 0%|  employed: 70|  unemployed: 10|  food: 215 days|  needs: 0% 0% 0%|  satisfaction: 0|  births 2, deaths 1 ^";
const BUILDINGS_ROWS: &str = "farm|  levels: mixed_farming 3|  positions: 25 filled, 5 open|  housing occupants: 0|  cattle: 27 / 36|  store: 1 / 3500 L|  wage: 0||storehouse|  levels: 1|  positions: 0 filled, 2 open|  housing occupants: 0|  store: 0 / 36000 L|  wage: 0";

#[test]
fn the_three_tabs_show_a_fixture_places_lines_as_literals_and_a_click_switches_between_them() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan')",
        "announce_tabs()",
        "announce_tabs()",
        "announce_tabs()",
    ]);
    rig.opened();
    // Selecting opens the panel on the Location tab.
    assert_eq!(rows(&rig), [LOCATION_ROWS]);
    rig.opened();
    click_tab(&mut rig, "pops");
    assert_eq!(rows(&rig), [LOCATION_ROWS, POPS_ROWS]);
    click_tab(&mut rig, "buildings");
    assert_eq!(rows(&rig), [LOCATION_ROWS, POPS_ROWS, BUILDINGS_ROWS]);
    // And back; a click on the tab already showing changes nothing.
    click_tab(&mut rig, "location");
    click_tab(&mut rig, "location");
    assert_eq!(
        rows(&rig),
        [LOCATION_ROWS, POPS_ROWS, BUILDINGS_ROWS, LOCATION_ROWS]
    );
}

#[test]
fn a_place_of_nobody_has_zeroes_and_empty_pops_and_buildings() {
    let mut rig = rig(&[
        "time(1, 1) select_place('nobody', 'Nobody')",
        "announce_tabs()",
    ]);
    rig.opened();
    assert_eq!(
        rows(&rig),
        [
            "people: 0|  workers: 0|  dependents: 0|housing: basic 0/0, makeshift 0/0|crowding: 0%|food: 0 days|building land: 0 / 0 m²|storage: 0 / 0 L|lost to overflow last month: 0|prices (base):"
        ]
    );
    rig.opened();
    click_tab(&mut rig, "pops");
    click_tab(&mut rig, "buildings");
    assert_eq!(&rows(&rig)[1..], ["no pops", "no buildings"]);
}

#[test]
fn the_panel_refreshes_on_a_new_revision_and_not_on_a_date_alone() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan')",
        // Only the date moves: the same revision.
        "time(2, 1) time(3, 1)",
        // A new revision refreshes the open panel from its own view.
        "time(12, 2)",
        "note('calls ' .. table.concat(place_calls, ','))",
    ]);
    rig.opened();
    rig.opened();
    assert_eq!(
        rows(&rig),
        [LOCATION_ROWS],
        "a date alone refreshes nothing"
    );
    rig.opened();
    let refreshed = LOCATION_ROWS.replace("people: 120|  workers: 80", "people: 130|  workers: 90");
    assert_eq!(rows(&rig), [LOCATION_ROWS.to_owned(), refreshed]);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("calls rowan@1,rowan@2"),
        "one read for the selection and one for the new revision, none for the dates"
    );
}

#[test]
fn a_selection_reads_the_newest_revision_the_ui_has_seen() {
    let mut rig = rig(&[
        "time(1, 1) time(12, 2) select_place('rowan', 'Rowan')",
        "note('calls ' .. table.concat(place_calls, ','))",
    ]);
    rig.opened();
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("calls rowan@2")
    );
}

#[test]
fn a_selection_before_any_view_says_so_and_reads_nothing() {
    let mut rig = rig(&[
        "select_place('rowan', 'Rowan')",
        "note('calls ' .. #place_calls)",
        // The first view arrives: the open panel fills in.
        "time(1, 1)",
    ]);
    rig.opened();
    assert_eq!(rows(&rig), ["economy: not published yet"]);
    rig.opened();
    assert_eq!(notes(&rig).last().map(String::as_str), Some("calls 0"));
    rig.opened();
    assert_eq!(rows(&rig).last().map(String::as_str), Some(LOCATION_ROWS));
}

#[test]
fn selecting_another_place_keeps_the_tab_showing() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan')",
        "announce_tabs()",
        "select_place('nobody', 'Nobody')",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    // The Pops tab is still showing for the other place.
    assert_eq!(rows(&rig).last().map(String::as_str), Some("no pops"));
}

#[test]
fn the_panel_fills_the_left_third_below_the_time_bar_and_follows_a_resize() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan') announce_geometry()",
        "announce_geometry()",
    ]);
    rig.opened();
    // 1800 points wide: the left third is 600 points; the time bar is 8 + 38 points down, and the panel sits a margin below it.
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("panel 0 54 600 846")
    );
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("panel 0 54 400 646")
    );
}

#[test]
fn a_window_taller_than_the_lists_row_limit_opens_and_resizes_with_a_usable_list() {
    let mut rig = Rig::new_fixture_body(&script(&[
        "time(1, 1) select_place('rowan', 'Rowan') announce_fit('Rowan')",
        "announce_fit('Rowan')",
    ]));
    // A row is 13 UI pixels, 26 points: 7,200 points leave room for more than the limit of 256 rows.
    rig.deliver_fixture_frame_at(1800, 7200, 1);
    rig.opened();
    assert_eq!(
        notes(&rig)
            .iter()
            .rev()
            .find(|line| line.starts_with("list_rows "))
            .map(String::as_str),
        Some("list_rows 256")
    );
    // A resize that keeps it that tall rebuilds the list without failing.
    rig.deliver_fixture_frame_at(1700, 7300, 1);
    rig.opened();
    assert_eq!(
        notes(&rig)
            .iter()
            .rev()
            .find(|line| line.starts_with("list_rows "))
            .map(String::as_str),
        Some("list_rows 256")
    );
}

#[test]
fn a_long_word_of_fada_bearing_letters_wraps_between_characters() {
    let mut rig = Rig::new_fixture_body(&script(&[
        "time(1, 1) select_place('fada', 'Fada')",
        "announce_tabs()",
    ]));
    // 300 points: the content is about 45 font pixels wide, so the 40 letters need many rows.
    rig.deliver_fixture_frame_at(300, 600, 2);
    rig.opened();
    rig.opened();
    // A byte cut inside a letter would have failed the callback when the row crossed to the host; here every letter survives.
    click_tab_at(&mut rig, "pops", (300, 600));
    let pops = rows(&rig).last().cloned().expect("the Pops rows");
    assert_eq!(pops.matches('\u{D3}').count(), 40, "{pops}");
    assert!(pops.split('|').count() > 10, "the word wrapped: {pops}");
}

/// The narrow layouts: below the least width that holds the tabs, the close button and the frame the panel is that width and extends
/// past the window; nothing raises, the title is empty when it has no room, and every control lies inside the panel, apart from the
/// others and clear of the title.
fn narrow_layout(width: u32, expect_floor: bool) {
    const NAME: &str = "Droim Ineasclainn";
    let scale = 2;
    let mut rig = Rig::new_fixture_body(&script(&[&format!(
        "time(1, 1) select_place('rowan', '{NAME}') announce_floor() announce_geometry() announce_fit('{NAME}')"
    )]));
    rig.deliver_fixture_frame_at(width, 800, 2);
    rig.opened();
    let line = |rig: &Rig, prefix: &str| {
        notes(rig)
            .into_iter()
            .rev()
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix}: {:?}", notes(rig)))
    };
    let floor: i64 = line(&rig, "floor ")
        .trim_start_matches("floor ")
        .parse()
        .unwrap();
    let panel: Vec<i64> = line(&rig, "panel ")
        .split_whitespace()
        .skip(1)
        .map(|part| part.parse().unwrap())
        .collect();
    let third = i64::from(width) / 3;
    assert!(
        panel[2] >= floor && panel[2] >= third - scale,
        "{panel:?} floor {floor}"
    );
    if expect_floor {
        assert!(
            floor > third,
            "the floor is above the third: {floor} > {third}"
        );
        assert_eq!(
            panel[2], floor,
            "the panel is its floor wide, and passes the window"
        );
    }
    // The whole name is kept; the tabs are inside the panel, apart from each other and the close button, and clear of the title.
    assert_eq!(
        line(&rig, "fit "),
        "fit title=true tabs_right=true title_clear=true"
    );
    assert_eq!(line(&rig, "tabs_left "), "tabs_left true");
    assert_eq!(line(&rig, "apart "), "apart true");
    if expect_floor {
        assert_eq!(line(&rig, "shown "), "shown ", "no room: no title");
    }
}

#[test]
fn in_a_narrow_window_the_panel_is_its_floor_and_passes_the_window() {
    narrow_layout(200, true);
}

#[test]
fn in_a_window_wide_enough_the_third_holds_the_controls() {
    narrow_layout(900, false);
}

// ---------------------------------------------------------------------------------------------
// The map-mode buttons (milestone 9, slice 8)
// ---------------------------------------------------------------------------------------------

/// The last announced centre of the mode button `key`, and whether it is selected.
fn mode_button(rig: &Rig, key: &str) -> (f64, f64, bool) {
    let prefix = format!("mode {key} ");
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no mode button {key}: {:?}", notes(rig)));
    let mut parts = line[prefix.len()..].split_whitespace();
    let x = parts.next().unwrap().parse().unwrap();
    let y = parts.next().unwrap().parse().unwrap();
    (x, y, parts.next().unwrap() == "true")
}

/// The modes the script has asked for, in order.
fn modes_called(rig: &Rig) -> Vec<String> {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("modes_called ").map(str::to_owned))
        .expect("modes were announced");
    line.split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The last announced rectangle `[x, y, width, height]`, in whole window points, of the button `key` ("stockpile", "yield",
/// "elevation" or "places").
fn bar_button(rig: &Rig, key: &str) -> [i64; 4] {
    let prefix = format!("bar {key} ");
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no bar button {key}: {:?}", notes(rig)));
    let numbers: Vec<i64> = line[prefix.len()..]
        .split_whitespace()
        .map(|part| part.parse().unwrap())
        .collect();
    [numbers[0], numbers[1], numbers[2], numbers[3]]
}

/// Presses the mode button `key` and delivers a frame.
fn press_mode(rig: &mut Rig, key: &str) {
    let (x, y, _) = mode_button(rig, key);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

#[test]
fn the_mode_buttons_publish_the_mode_and_a_second_press_turns_it_off() {
    let mut rig = rig(&[
        "time(1, 1)",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
    ]);
    rig.opened();
    rig.opened();
    // Nothing is asked for, and nothing is selected, until a button is pressed.
    assert_eq!(modes_called(&rig), Vec::<String>::new());
    assert!(!mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2);
    press_mode(&mut rig, "stockpile");
    rig.opened();
    assert_eq!(modes_called(&rig), ["stockpile"]);
    assert!(
        mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2,
        "the mode that shows is selected"
    );
    // The same button again turns the mode off.
    press_mode(&mut rig, "stockpile");
    rig.opened();
    assert_eq!(modes_called(&rig), ["stockpile", "off"]);
    assert!(!mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2);
    // The other button changes the mode with no off between.
    press_mode(&mut rig, "yield");
    press_mode(&mut rig, "stockpile");
    rig.opened();
    assert_eq!(
        modes_called(&rig),
        ["stockpile", "off", "yield", "stockpile"]
    );
    assert!(mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2);
}

#[test]
fn the_elevation_button_publishes_the_elevation_mode_beside_the_other_two() {
    let mut rig = rig(&[
        "time(1, 1)",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
    ]);
    rig.opened();
    rig.opened();
    let selected =
        |rig: &Rig| ["stockpile", "yield", "elevation"].map(|key| mode_button(rig, key).2);
    assert_eq!(selected(&rig), [false; 3]);
    // The three buttons are in a row, left to right, on one line.
    let (stockpile, yields, elevation) = (
        mode_button(&rig, "stockpile"),
        mode_button(&rig, "yield"),
        mode_button(&rig, "elevation"),
    );
    assert!(stockpile.0 < yields.0 && yields.0 < elevation.0);
    assert!(
        (stockpile.1 - yields.1).abs() < 0.5 && (yields.1 - elevation.1).abs() < 0.5,
        "one line"
    );
    press_mode(&mut rig, "elevation");
    rig.opened();
    assert_eq!(modes_called(&rig), ["elevation"]);
    assert_eq!(selected(&rig), [false, false, true], "only elevation shows");
    // Another mode replaces it, with no off between; the elevation button again turns it off.
    press_mode(&mut rig, "yield");
    press_mode(&mut rig, "elevation");
    press_mode(&mut rig, "elevation");
    rig.opened();
    assert_eq!(
        modes_called(&rig),
        ["elevation", "yield", "elevation", "off"]
    );
    assert_eq!(selected(&rig), [false; 3]);
}

#[test]
fn a_mode_the_lane_refuses_leaves_the_buttons_and_the_mode_as_they_were() {
    let mut rig = rig(&[
        "time(1, 1)",
        "fail_mode = true announce_modes()",
        "announce_modes()",
        "fail_mode = false announce_modes()",
        "announce_modes()",
    ]);
    rig.opened();
    rig.opened();
    press_mode(&mut rig, "yield");
    rig.opened();
    // The refused press changed nothing: no mode was set, and no button is selected.
    assert_eq!(modes_called(&rig), Vec::<String>::new());
    assert!(!mode_button(&rig, "yield").2 && !mode_button(&rig, "stockpile").2);
    // Once the lane takes it, the same press works.
    rig.opened();
    press_mode(&mut rig, "yield");
    rig.opened();
    assert_eq!(modes_called(&rig), ["yield"]);
    assert!(mode_button(&rig, "yield").2);
}

/// The map-mode bar (milestone 9, slice 12a) at 2 points a pixel, where `bar` is its top-left corner: the bar is 55 UI pixels wide (5
/// padding, three buttons of 13 with 3 between, 5 padding) and 19 tall, so 110 by 38 points. Its buttons are 13 by 15 pixels, the first
/// at the bar's `x + 5 P`, the three a button and 3 pixels apart, all at the bar's `y + 2 P`.
fn assert_mode_bar_at(rig: &Rig, bar: [i64; 2]) {
    const P: i64 = 2;
    let [stockpile, yields, elevation] =
        ["stockpile", "yield", "elevation"].map(|key| bar_button(rig, key));
    assert_eq!(
        stockpile,
        [bar[0] + 5 * P, bar[1] + 2 * P, 13 * P, 15 * P],
        "the stockpile button"
    );
    assert_eq!(
        yields,
        [bar[0] + (5 + 13 + 3) * P, stockpile[1], 13 * P, 15 * P],
        "the yield button, a button and a gap on"
    );
    assert_eq!(
        elevation,
        [
            bar[0] + (5 + 2 * (13 + 3)) * P,
            stockpile[1],
            13 * P,
            15 * P
        ],
        "the elevation button"
    );
}

#[test]
fn the_mode_bar_is_centred_at_the_bottom_of_the_window_and_follows_a_resize() {
    let mut rig = rig(&["time(1, 1) announce_bars()", "announce_bars()"]);
    rig.opened();
    // 1800 by 900 points. The bar is 110 by 38: centred it starts at (1800 - 110) / 2 = 845, which snaps to the 2-point grid as 846, and its
    // bottom edge is a margin of 8 above the window's: y = 900 - 38 - 8 = 854.
    assert_mode_bar_at(&rig, [846, 854]);
    let stockpile = bar_button(&rig, "stockpile");
    assert_eq!(stockpile[0], 846 + 5 * 2, "the first button's left edge");
    assert_eq!(
        stockpile[1] + stockpile[3] / 2,
        854 + 19,
        "the row's vertical centre"
    );
    let elevation = bar_button(&rig, "elevation");
    let left_gap = stockpile[0] - 5 * 2;
    let right_gap = 1800 - (elevation[0] + elevation[2] + 5 * 2);
    assert!(
        (left_gap - right_gap).abs() <= 2,
        "centred: {left_gap} points on the left, {right_gap} on the right"
    );
    // 1200 by 700: (1200 - 110) / 2 = 545, snapped 546; y = 700 - 38 - 8 = 654. The buttons moved with the bar.
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [546, 654]);
}

#[test]
fn the_places_bar_stays_at_the_top_right_beside_the_bottom_mode_bar() {
    let mut rig = rig(&["time(1, 1) announce_bars()", "announce_bars()"]);
    rig.opened();
    // The places button sits in its bar, which is a margin of 8 from the top and the right edges: its top is 8 + 2 P, and its right edge
    // plus the bar's padding of 5 P plus the margin reaches the window's right edge (to a snap).
    let check = |rig: &Rig, width: i64| {
        let places = bar_button(rig, "places");
        assert_eq!(places[1], 8 + 2 * 2, "the top of the places button");
        assert!(
            (places[0] + places[2] + 5 * 2 + 8 - width).abs() <= 2,
            "the right edge of the places bar: {places:?} in {width}"
        );
        assert!(
            bar_button(rig, "stockpile")[1] > places[1] + places[3],
            "the mode bar is below the places bar, not beside it"
        );
    };
    check(&rig, 1800);
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    check(&rig, 1200);
}

/// The shipped script opens the window resizable, at 1280 by 800 screen pixels, with that minimum (milestone 9, slice 12b): this
/// rig has no screen snapshot, so the pixels resolve one to one and the description keeps the request it was made from.
#[test]
fn the_shipped_script_opens_a_resizable_window_of_1280_by_800_pixels_with_that_minimum() {
    let rig = rig(&["time(1, 1)"]);
    let desc = rig
        .harness
        .requests
        .reader()
        .take()
        .open
        .expect("the shipped script opened its window");
    assert!(desc.resizable);
    assert_eq!(desc.unit, ck_window::SizeUnit::Points, "resolved");
    assert_eq!((desc.width, desc.height), (1280, 800));
    assert_eq!(desc.min, Some((1280, 800)));
    assert_eq!(
        desc.pixel_request,
        Some(ck_window::PixelRequest {
            size: (1280, 800),
            min: Some((1280, 800)),
        }),
        "asked for in pixels, with the minimum in pixels"
    );
}

/// A window opened in pixels: the UI VM is in UI units at the scale its drawable allows, so the same 1280 by 800 view comes from
/// 1280 by 800 pixels at scale 1 and from 1920 by 1200 at 1.5, and the shipped script's relayout keeps the map-mode bar at the bottom
/// centre of it; a window of 3456 by 2232 pixels at scale 2 has a 1728 by 1116 view.
#[test]
fn a_change_of_the_ui_scale_is_seen_as_a_resize_and_the_script_relays_out_in_units() {
    let mut rig = Rig::new_fixture_body_in_pixels(&script(&[
        "time(1, 1) announce_bars()",
        "announce_bars()",
        "announce_bars()",
    ]));
    // 640x400 points at 2 pixels a point: a drawable of 1280x800 and scale 1: a view of 1280x800 units. The bar is 110x38 units, centred
    // (585, snapped to 586) and 8 above the bottom edge (800 - 38 - 8 = 754).
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [586, 754]);
    // 960x600 points: 1920x1200 pixels, the scale 1.5 and the same 1280x800 view: the layout is unchanged.
    rig.deliver_fixture_frame_at(960, 600, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [586, 754]);
    // 1728x1116 points: 3456x2232 pixels, the scale 2 and a 1728x1116 view: (1728 - 110) / 2 = 809 snaps to 810; 1116 - 38 - 8 = 1070.
    rig.deliver_fixture_frame_at(1728, 1116, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [810, 1070]);
}

/// At 3456 by 2232 pixels on a 2x display the scale is 2 and a UI unit is a point: every quad of the shipped start is where the build with
/// a UI pixel of 2 points drew it, in the units and on the pixels.
#[test]
fn at_the_scale_two_the_shipped_start_lands_on_the_pixels_of_a_points_window() {
    let steps = ["time(1, 1) select_place('rowan', 'Rowan')"];
    let mut points = Rig::new_fixture_body(&script(&steps));
    let mut pixels = Rig::new_fixture_body_in_pixels(&script(&steps));
    for rig in [&mut points, &mut pixels] {
        rig.deliver_fixture_frame_at(1728, 1116, 2);
        rig.opened();
    }
    let rects = |rig: &Rig| {
        rig.sink
            .quads()
            .iter()
            .map(|quad| (quad.x, quad.y, quad.width, quad.height))
            .collect::<Vec<_>>()
    };
    assert!(rects(&points).len() > 20, "the start has quads");
    assert_eq!(rects(&points), rects(&pixels), "the same units");
    let drawn = |rig: &Rig, scale_k: Option<u32>| {
        let metrics = ck_render_2d::Metrics {
            points_width: 1728,
            points_height: 1116,
            pixels: ck_window::PixelSize {
                width: 3456,
                height: 2232,
            },
            ui_scale_k: scale_k,
        };
        rig.sink
            .quads()
            .iter()
            .map(|quad| {
                quad.place(&metrics)
                    .map(|placed| format!("{:?}", placed.destination))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        drawn(&points, None),
        drawn(&pixels, Some(4)),
        "the same pixels"
    );
}
