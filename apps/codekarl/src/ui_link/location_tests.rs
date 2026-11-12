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
local function price(good, milli, base, bought, offered)
  return {good = good, price_milli = milli, base = base, bought = bought, offered = offered}
end
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
fada.pops[1].kind = "mystery"
fada.pops[2] = {kind = "monk", culture = "mide", religion = "christianity", workers = 2, dependents = 3, employed = 2, unemployed = 0,
  food_days = 40, necessities = 0, niceties = 0, luxuries = 0, satisfaction = 0, births = 0, deaths = 0}
local long_place = rowan(0, 0)
long_place.id = "long"
local cards_place = rowan(108, 2)
cards_place.id = "cards"
cards_place.dependents = 72
cards_place.pops = {
  {kind = "peasant", culture = "ulaid", religion = "christianity", workers = 108, dependents = 72, employed = 96, unemployed = 12,
    food_days = 74, necessities = 1000, niceties = 620, luxuries = 80, satisfaction = 12, births = 2, deaths = 1,
    basic_housing = 380, makeshift_housing = 32, crowding_pm = 950},
  {kind = "monk", culture = "mide", religion = "christianity", workers = 0, dependents = 5, employed = 0, unemployed = 0,
    food_days = 300, necessities = 1200, niceties = 0, luxuries = 0, satisfaction = -3, births = 0, deaths = 2,
    basic_housing = 0, makeshift_housing = 0, crowding_pm = false},
}
cards_place.buildings = {
  {kind = "farm", levels = 8, methods = {{id = "mixed_farming", levels = 8}, {id = "coastal", levels = 0}}, filled = 80, open = 8,
    herds = {{species = "cattle", head = 27, maximum = 36}, {species = "sheep", head = 10, maximum = 20},
      {species = "pigs", head = 9, maximum = 9}}, store_used_l = 6400, store_capacity_l = 8000, wage = 1234,
    housing_occupants = 3},
  {kind = "holdings", levels = 1, methods = {{id = "strip_farming", levels = 1}}, filled = 0, open = 0,
    herds = {}, store_used_l = 0, store_capacity_l = 0, wage = 92, housing_occupants = 0},
  {kind = "storehouse", levels = 1, methods = {}, filled = 0, open = 0,
    herds = {}, store_used_l = 0, store_capacity_l = 0, wage = 0, housing_occupants = 0},
  {kind = "holdings", levels = 1, methods = {{id = "strip_farming", levels = 1}}, filled = 8, open = 0,
    herds = {}, store_used_l = 9000, store_capacity_l = 8000, wage = 92, housing_occupants = 0},
  {kind = "granary", levels = 1, methods = {}, filled = 0, open = 0,
    herds = {}, store_used_l = 0, store_capacity_l = 8000, wage = 0, housing_occupants = 0},
}
local scrolling_place = rowan(20, 0)
scrolling_place.id = "scrolling"
scrolling_place.pops = {}
for index = 1, 60 do
  scrolling_place.pops[index] = {kind = "peasant", culture = "ulaid", religion = "christianity", workers = 20, dependents = 4,
    employed = 18, unemployed = 2, food_days = 40, necessities = 1000, niceties = 200, luxuries = 0,
    satisfaction = 0, births = 0, deaths = 0}
end
VIEWS = {
  [1] = {rowan = rowan(80, 2), nobody = nobody, fada = fada, long = long_place, cards = cards_place, scrolling = scrolling_place, before = nobody},
  [2] = {rowan = rowan(90, 0), nobody = nobody, cards = cards_place, scrolling = scrolling_place, fada = fada, long = long_place, before = nobody},
}
place_data = {
  {id = "rowan", name = "Rowan", kind = "town", size = 3},
  {id = "nobody", name = "No Economy", kind = "town", size = 1},
  {id = "fada", name = "Cluain Mhic Nóis", kind = "monastery", size = 2},
  {id = "long", name = string.rep("\u{D3}", 40), kind = "town", size = 1},
}
game = {
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  places = function() return place_data end,
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
local set_frame_update = frame.on_update
frame.on_update = function(callback)
  if callback == nil then return set_frame_update(nil) end
  return set_frame_update(function(update)
    callback(update)
    if frame_test_observer then frame_test_observer(update) end
  end)
end
local buttons_library = require("lib.button")
local real_button_new = buttons_library.new
mode_buttons = {}
bar_buttons = {}
look_buttons = {}
time_buttons = {}
places_text_buttons = 0
local bar_group = nil
local bar_visible = true
buttons_library.new = function(spec)
  local created = real_button_new(spec)
  if spec.text == "places" then places_text_buttons = places_text_buttons + 1 end
  if spec.icon == "location" or spec.icon == "pops" then
    bar_buttons[spec.icon] = created
  end
  if spec.icon == "cards" or spec.icon == "rows" then look_buttons[spec.icon] = created end
  if spec.icon == "rewind" or spec.icon == "pause" or spec.icon == "fast_forward" then time_buttons[spec.icon] = created end
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
-- Each mode button's and ledger icon's left edge, top edge, width and height, in window points.
function announce_bars()
  note("places_text_buttons " .. places_text_buttons)
  for _, key in ipairs({"stockpile", "yield", "elevation"}) do
    local button = mode_buttons[key]
    local width, height = button:size()
    note(string.format("bar %s %g %g %g %g", key, button.x, button.y, width, height))
  end
  for _, key in ipairs({"location", "pops"}) do
    local button = bar_buttons[key]
    local width, height = button:size()
    note(string.format("bar %s %g %g %g %g", key, button.x, button.y, width, height))
  end
end
function announce_look()
  local origin_x = panel_window.x + 6
  local origin_y = panel_window.y + panel_window.title_height + 6
  for _, key in ipairs({"cards", "rows"}) do
    local control = look_buttons[key]
    local width, height = control:size()
    note(string.format("look %s %g %g %g %g selected=%s", key, origin_x + control.x, origin_y + control.y, width, height, tostring(control.selected == true)))
  end
end
function announce_selector_liveness()
  note("selector_live " .. tostring(look_buttons.cards ~= nil and look_buttons.cards.frame ~= nil
    and look_buttons.rows ~= nil and look_buttons.rows.frame ~= nil))
end
selector_objects = {}
local function attached_selector_count(window_object)
  local selectors = selector_objects[window_object] or {}
  local count = 0
  for _, attached in ipairs(window_object.attached) do
    for _, selector in ipairs(selectors) do
      if attached == selector then
        count = count + 1
        break
      end
    end
  end
  return count
end
function announce_selector_count()
  note("selector_count " .. attached_selector_count(panel_window))
end
function note(text) window.record(text) end
tooltip_sources = {}
local tooltip = require("lib.tooltip")
local real_tooltip_attach = tooltip.attach
tooltip.attach = function(key, source)
  tooltip_sources[key] = source
  return real_tooltip_attach(key, source)
end
local real_tooltip_detach = tooltip.detach
tooltip.detach = function(key)
  tooltip_sources[key] = nil
  return real_tooltip_detach(key)
end
function note_tooltip(name, key, x, y)
  local source = tooltip_sources[key]
  if source == nil then note("tooltip " .. name .. " <none>"); return end
  local content, part
  if type(source) == "function" then content, part = source(x or 0, y or 0)
  else content = tooltip.content(source.class, source.args) end
  local function line_text(line)
    if type(line) ~= "table" then return tostring(line) end
    if type(line.text) == "string" then return line.text end
    local parts = {}
    for _, span in ipairs(line) do parts[#parts + 1] = line_text(span) end
    return table.concat(parts)
  end
  local lines = {}
  for _, line in ipairs(content or {}) do lines[#lines + 1] = line_text(line) end
  note("tooltip " .. name .. " " .. table.concat(lines, "|") .. " part=" .. tostring(part or "-"))
end
function note_tooltip_links(name, key)
  local source = tooltip_sources[key]
  if type(source) ~= "function" then note("tooltip_link " .. name .. " <none>"); return end
  local content = source(0, 0)
  for _, line in ipairs(content or {}) do
    if type(line) == "table" then
      for _, span in ipairs(line) do
        if type(span) == "table" and span.class ~= nil then
          note("tooltip_link " .. name .. " " .. span.class .. " " .. span.args.place .. " " .. span.args.good .. " " .. span.text)
          return
        end
      end
    end
  end
  note("tooltip_link " .. name .. " <none>")
end
function note_price_content(name, place, good)
  tooltip.refresh()
  local lines = tooltip.content("price", {place = place, good = good})
  local text = {}
  for _, line in ipairs(lines or {}) do text[#text + 1] = type(line) == "string" and line or table.concat(line) end
  note("price_content " .. name .. " " .. table.concat(text, "|"))
end
local drawing_card = false
local ui_library = require("lib.ui")
local real_ui_texture = ui_library.texture
local texture_names = {}
ui_library.texture = function(name)
  local token = real_ui_texture(name)
  texture_names[token] = name
  return token
end
local real_create_quad = render.create_quad
render.create_quad = function(spec)
  local name = texture_names[spec.texture]
  if name ~= nil and name:match("^ui/goods/") then
    note(string.format("goodsquad %s %g %g %g %g %s", name, spec.x, spec.y, spec.width, spec.height, tostring(spec.tint or "-")))
  end
  if drawing_card then
    note(string.format("cardquad %s %g %g %g %g %s", tostring(spec.texture), spec.x, spec.y,
      spec.width, spec.height, tostring(spec.tint or "-")))
  end
  return real_create_quad(spec)
end
local text_library = require("lib.text")
local real_text_set = text_library.set
text_library.set = function(line, content)
  real_text_set(line, content)
  if drawing_card then line.ck_test_text = content end
end
local real_text_move = text_library.move
text_library.move = function(line, x, y)
  real_text_move(line, x, y)
  if drawing_card and line.ck_test_text ~= nil then
    note(string.format("cardtext %g %g %d %s %s", line.x, line.y, line.width, line.colour, line.ck_test_text))
  end
end
local real_group_visible = render.set_group_visible
local real_set_hit = render.set_hit
render.set_hit = function(key, rects, group)
  if key == "places_bar" then bar_group = group end
  return real_set_hit(key, rects, group)
end
render.set_group_visible = function(group, visible)
  if group == bar_group then bar_visible = visible end
  return real_group_visible(group, visible)
end
function time(hour, revision, paused)
  callbacks.time({year = 768, month = 1, day = 1, hour = hour, paused = paused == true, speed = 1, economy_revision = revision})
end
function select_place(id, name) callbacks.select({id = id, name = name, kind = "town", size = 3}) end

-- Every list's rows, as created and as replaced.
local lists = require("lib.list")
local real_list_new = lists.new
local function row_text(item)
  if type(item) == "string" then return item end
  local cells = {}
  for _, cell in ipairs(item.cells) do
    cells[#cells + 1] = string.format("%s@%g:%s:%s", cell.text, cell.x, cell.align, cell.colour or "-")
  end
  local icon = item.icon and (item.icon.image or item.icon.name) or "-"
  return table.concat(cells, ",") .. ";stripe=" .. (item.stripe or "-") .. ";icon=" .. icon
end
function rows_text(items)
  local rows = {}
  for index, item in ipairs(items) do rows[index] = row_text(item) end
  return table.concat(rows, "|")
end
lists.new = function(spec)
  local created = real_list_new(spec)
  note("rows " .. rows_text(created.items))
  local real_set_items = created.set_items
  function created:set_items(items)
    note("rows " .. rows_text(items))
    real_set_items(self, items)
  end
  list_object = created
  return created
end
local cards_library = require("lib.cards")
local real_cards_new = cards_library.new
local function observe_card_items(items)
  for _, item in ipairs(items) do
    local draw = item.draw
    item.draw = function(...)
      drawing_card = true
      local result = draw(...)
      drawing_card = false
      return result
    end
  end
end
local function cards_summary(object)
  local heights = {}
  for index, item in ipairs(object.items) do heights[index] = tostring(item.height) end
  note("cardstack gap=" .. object.gap .. " heights=" .. table.concat(heights, ","))
end
cards_library.new = function(spec)
  observe_card_items(spec.items)
  card_object = real_cards_new(spec)
  cards_summary(card_object)
  local real_set_items = card_object.set_items
  function card_object:set_items(items, gap)
    observe_card_items(items)
    real_set_items(self, items, gap)
    cards_summary(self)
  end
  return card_object
end
local tabs_library = require("lib.tabs")
local real_tabs_new = tabs_library.new
tabs_by_title = {}
tabs_library.new = function(spec)
  tabs_object = real_tabs_new(spec)
  tabs_by_title[spec.window.title] = tabs_object
  return tabs_object
end
local windows = require("lib.window")
local real_windows_new = windows.new
window_objects = {}
windows.new = function(spec)
  panel_window = real_windows_new(spec)
  window_objects[spec.title] = panel_window
  selector_objects[panel_window] = {}
  local real_attach = panel_window.attach
  function panel_window:attach(item)
    if type(item.buttons) == "table" and item.buttons.cards ~= nil and item.buttons.rows ~= nil then
      table.insert(selector_objects[self], item)
    end
    return real_attach(self, item)
  end
  return panel_window
end
-- The centre of each tab button, in window points, for the driver to click.
function announce_tabs()
  local x, y = panel_window:position()
  for index, item in ipairs(tabs_object.items) do
    local width, height = tabs_object.buttons[index]:size()
    note(string.format("tab %s %d %d %s", item.key, x + tabs_object.buttons[index].x + width / 2,
      y + tabs_object.buttons[index].y + height / 2,
      (tabs_object.buttons[index].text or "") .. " selected=" .. tostring(tabs_object.buttons[index].selected == true)))
  end
end
function announce_ledger()
  local ledger = window_objects.ledger
  if ledger == nil or ledger.closed then
    note("ledger closed bar_visible=" .. tostring(bar_visible))
    return
  end
  local x, y = ledger:position()
  local width, height = ledger:size()
  local content_x, content_y = ledger:content_origin()
  note(string.format("ledger %g %g %g %g content=%g %g draggable=%s bar_visible=%s tab=%s",
    x, y, width, height, ledger.content_width, ledger.content_height, tostring(ledger.draggable), tostring(bar_visible),
    tabs_object:current()))
  note(string.format("ledger_origin %g %g", content_x, content_y))
  local close = ledger.close_button
  if close ~= nil then
    note(string.format("ledger_close %g %g", x + close.x + close.width / 2, y + close.y + close.height / 2))
  end
end
function announce_ledger_tabs()
  panel_window = window_objects.ledger
  tabs_object = tabs_by_title.ledger
  announce_tabs()
end
function announce_place_tabs()
  panel_window = window_objects.Rowan
  tabs_object = tabs_by_title.Rowan
  announce_tabs()
  note("place_selected " .. tabs_object:current())
  note("place_cards_open " .. tostring(card_object ~= nil and not card_object.closed))
end
function announce_active_panel()
  note(string.format("active_panel %s %s ledger_closed=%s", panel_window.title, tabs_object:current(),
    tostring(window_objects.ledger == nil or window_objects.ledger.closed)))
  note("place_calls " .. table.concat(place_calls, ","))
end
function announce_last_place_call() note("last_place_call " .. tostring(place_calls[#place_calls])) end
function announce_pop_clip()
  for _, item in ipairs(list_object.items) do
    if item.icon ~= nil and item.icon.name == "pops" then
      local cell = item.cells[1]
      if cell.text:sub(-2) == ".." then
        note(string.format("pop_clip x=%d right=%d", cell.x, cell.x + font.layout(cell.text).width))
        return
      end
    end
  end
  error("no clipped pop label")
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

fn card_stacks(rig: &Rig) -> Vec<String> {
    notes(rig)
        .into_iter()
        .filter_map(|line| line.strip_prefix("cardstack ").map(str::to_owned))
        .collect()
}

fn look_point(rig: &Rig, key: &str) -> (f64, f64) {
    let prefix = format!("look {key} ");
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no look button {key}: {:?}", notes(rig)));
    let mut parts = line[prefix.len()..].split_whitespace();
    let x = parts.next().unwrap().parse::<f64>().unwrap();
    let y = parts.next().unwrap().parse::<f64>().unwrap();
    (x + 13.0, y + 15.0)
}

fn click_look(rig: &mut Rig, key: &str) {
    let (x, y) = look_point(rig, key);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

fn click_panel_point(rig: &mut Rig, x: f64, y: f64) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
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

#[test]
fn a_pops_selector_is_detached_before_a_resize_refit_and_keeps_look_and_scroll() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs()",
        "announce_look()",
        "card_object:set_pos(26); note('before_scroll ' .. card_object:scroll_position()); announce_look(); announce_selector_count()",
        "note('after_scroll ' .. card_object:scroll_position()); announce_look(); announce_selector_count()",
        "select_place('scrolling', 'Scrolling'); announce_tabs(); announce_selector_count()",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    rig.opened();
    rig.deliver_fixture_frame_at(1700, 800, 2);
    rig.opened();

    let recorded = notes(&rig);
    assert!(recorded.iter().any(|line| line == "before_scroll 26"));
    assert!(recorded.iter().any(|line| line == "after_scroll 26"));
    assert!(
        recorded
            .iter()
            .any(|line| line.starts_with("look rows ") && line.ends_with("selected=true"))
    );
    assert!(
        recorded
            .iter()
            .filter(|line| *line == "selector_count 1")
            .count()
            >= 2
    );

    assert!(rig.escape(), "Escape closes the refitted location panel");
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "selector_count 1"));
}

#[test]
fn a_location_tab_detaches_its_selector_before_resize_and_selection_change() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs()",
        "note('before_resize_selector_count ' .. attached_selector_count(panel_window))",
        "select_place('cards', 'Cards'); note('after_selection_selector_count ' .. attached_selector_count(panel_window))",
        "select_place('cards', 'Cards'); note('reopened_selector_count ' .. attached_selector_count(panel_window))",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    click_tab(&mut rig, "location");
    rig.opened();
    rig.deliver_fixture_frame_at(1700, 800, 2);
    rig.opened();

    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "before_resize_selector_count 0")
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "after_selection_selector_count 0")
    );
    assert!(rig.escape(), "Escape closes the location-only panel");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "reopened_selector_count 0")
    );
}

#[test]
fn switching_location_tabs_keeps_one_selector_and_close_reopen_starts_clean() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs(); note('selector_count ' .. attached_selector_count(panel_window))",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "select_place('scrolling', 'Scrolling'); announce_selector_count()",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_tab(&mut rig, "location");
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_tab(&mut rig, "buildings");
    rig.opened();
    click_tab(&mut rig, "location");
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();

    let counts = notes(&rig)
        .iter()
        .filter_map(|line| line.strip_prefix("selector_count "))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(counts, ["0", "1", "0", "1", "1", "0", "1"]);

    assert!(
        rig.escape(),
        "Escape closes the tab-switched location panel"
    );
    rig.opened();
    assert_eq!(
        notes(&rig)
            .iter()
            .filter_map(|line| line.strip_prefix("selector_count "))
            .next_back(),
        Some("1")
    );
}

const LOCATION_ROWS: &str = "people: 120|  workers: 80|  dependents: 40|housing: basic 0/0, makeshift 0/0|crowding: 0%|food: 215 days|building land: 4200 / 24200 m²|storage: 5400 / 86400 L|lost to overflow last month: 0|prices (base):|100 (100)@16:left:-;stripe=-;icon=goods/oats|2.5 (2)@16:left:-;stripe=-;icon=goods/ale";

#[test]
fn prices_use_fixed_goods_icons_and_unlisted_goods_keep_their_name() {
    let mut rig = rig(&[
        "VIEWS[1].rowan.prices = {price('oats', 96500, 100), price('peas_beans', 1200, 2), price('seabirds', 750, 1), price('moonstone', 4500, 4)}; time(1, 1) select_place('rowan', 'Rowan'); note_tooltip('oats', list_object.row_keys[11]); note_tooltip('peas_beans', list_object.row_keys[12]); note_tooltip('seabirds', list_object.row_keys[13])",
    ]);
    rig.opened();
    let recorded = notes(&rig);
    let all_rows = rows(&rig);
    let price_rows = all_rows.last().expect("location rows");
    assert!(
        price_rows.contains("96.5 (100)@16:left:-;stripe=-;icon=goods/oats"),
        "a listed good replaces its name with the 9x9 icon and x=16 price cell: {price_rows}"
    );
    assert!(
        price_rows.contains("  moonstone 4.5 (4)"),
        "an unlisted good keeps the prior name line: {price_rows}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line.starts_with("goodsquad ui/goods/oats.png 8 ")
                && line.ends_with(" 18 18 -")),
        "the price icon uses the full-colour image at its row position: {recorded:?}"
    );
    assert!(
        !recorded
            .iter()
            .any(|line| line.contains("ui/goods/moonstone.png")),
        "an unlisted good does not attempt a texture load: {recorded:?}"
    );
    for (name, expected) in [
        ("oats", "oats|price 96.5, base 100"),
        ("peas_beans", "peas and beans|price 1.2, base 2"),
        ("seabirds", "seabirds|price 0.75, base 1"),
    ] {
        assert!(
            recorded
                .iter()
                .any(|line| line == &format!("tooltip {name} {expected} part=-")),
            "missing tooltip {name}: {recorded:?}"
        );
    }
}

#[test]
fn price_tooltip_links_to_a_refreshing_four_line_breakdown() {
    let mut rig = rig(&[
        r#"VIEWS[1].rowan.prices = {price("oats", 96500, 100)}; time(1, 1); select_place("rowan", "Rowan")
note_tooltip("oats", list_object.row_keys[11]); note_tooltip_links("oats", list_object.row_keys[11])
note_price_content("empty", "rowan", "oats")
VIEWS[2] = VIEWS[1]; VIEWS[2].rowan.prices = {price("oats", 97250, 100, 1240, 1410)}; time(2, 2)
note_price_content("updated", "rowan", "oats")"#,
    ]);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip oats oats|price 96.5, base 100 part=-"),
        "row tooltip: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_link oats price rowan oats 96.5"),
        "price span link: {recorded:?}"
    );
    assert!(recorded.iter().any(|line| line == "price_content empty oats: price 96.5|base price 100|bought (buy orders) -|offered (sell orders) -"), "empty breakdown: {recorded:?}");
    assert!(recorded.iter().any(|line| line == "price_content updated oats: price 97.25|base price 100|bought (buy orders) 1240|offered (sell orders) 1410"), "refreshed breakdown: {recorded:?}");
}

#[test]
fn built_icon_buttons_and_tabs_have_their_spec_tooltip_labels() {
    let mut rig = rig(&[
        r#"time(1, 1); select_place('rowan', 'Rowan'); tabs_by_title.Rowan:select('pops', true)
for _, key in ipairs({"location", "pops"}) do note_tooltip("places_" .. key, bar_buttons[key].key) end
for _, key in ipairs({"stockpile", "yield", "elevation"}) do note_tooltip("mode_" .. key, mode_buttons[key].key) end
for _, key in ipairs({"rewind", "fast_forward"}) do note_tooltip("time_" .. key, time_buttons[key].key) end
for index = 1, 5 do note_tooltip("speed_" .. index, "speed_" .. index) end
for _, key in ipairs({"cards", "rows"}) do note_tooltip("look_" .. key, look_buttons[key].key) end
note_tooltip("location_full", tabs_by_title.Rowan.buttons[1].key)
bar_buttons.location.on_click()
for index, tab in ipairs(tabs_by_title.ledger.items) do note_tooltip("ledger_" .. tab.key, tabs_by_title.ledger.buttons[index].key) end"#,
        r#"time(1, 1, true); note_tooltip("time_pause", time_buttons.pause.key)"#,
        r#"note("location_tier " .. tabs_by_title.Rowan.tier); note_tooltip("location_short", tabs_by_title.Rowan.buttons[1].key)"#,
    ]);
    rig.opened();
    let recorded = notes(&rig);
    for (name, expected) in [
        ("places_location", "places"),
        ("places_pops", "pops"),
        ("mode_stockpile", "stockpile map"),
        ("mode_yield", "yield map"),
        ("mode_elevation", "elevation map"),
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
    ] {
        assert!(
            recorded
                .iter()
                .any(|line| line == &format!("tooltip {name} {expected} part=-")),
            "missing {name}={expected}: {recorded:?}"
        );
    }
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
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the real price-row witness verifies tooltip delay, passive input, row removal and relayout in one UI lifetime"
)]
fn real_price_row_hover_is_passive_survives_resize_and_leaves_no_old_keys() {
    let mut rig = rig(&[
        r"VIEWS[1].rowan.prices = {price('oats', 96500, 100), price('moonstone', 4500, 4)}
time(1, 1); select_place('rowan', 'Rowan')
local text = require('lib.text'); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tips then note('tooltip_text ' .. value) end end
capture_tips = true
frame_test_observer = function()
  note('list_hover ' .. tostring(list_object.hover_row) .. ' pressed ' .. tostring(list_object.pressed_row))
end
old_price_key = list_object.row_keys[11]
local x = list_object.origin_x + list_object.x + 10
local y = list_object.origin_y + list_object.y + 10 * lists.ROW + 10
note(string.format('oats_point %g %g', x, y)); note('old_price_key ' .. old_price_key)",
        r"local x = list_object.origin_x + list_object.x + 10
local y = list_object.origin_y + list_object.y + 10 * lists.ROW + 10
note('old_key_live ' .. tostring(require('lib.pointer').widget(old_price_key) ~= nil))
note(string.format('resized_oats_point %g %g', x, y)); note('resized_price_key ' .. list_object.row_keys[11])",
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 1);
    rig.opened();

    let recorded = notes(&rig);
    let point = recorded
        .iter()
        .find(|line| line.starts_with("oats_point "))
        .unwrap();
    let mut coords = point
        .split_whitespace()
        .skip(1)
        .map(|value| value.parse::<f64>().unwrap());
    let price_point = (coords.next().unwrap(), coords.next().unwrap());
    let old_price_key = recorded
        .iter()
        .find_map(|line| line.strip_prefix("old_price_key "))
        .unwrap();
    assert_eq!(
        rig.key_at(price_point.0, price_point.1).as_deref(),
        Some(old_price_key)
    );
    rig.feed(&[PointerEvent::Move(points(price_point.0, price_point.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 1);
    }
    let opened = notes(&rig);
    assert!(
        opened.iter().any(|line| line == "tooltip_text oats"),
        "tooltip did not open: {opened:?}"
    );
    let spans = [
        "tooltip_text oats",
        "tooltip_text price ",
        "tooltip_text 96.5",
        "tooltip_text , base ",
        "tooltip_text 100",
    ];
    assert!(
        opened.windows(spans.len()).any(|window| window == spans),
        "price tooltip did not open the expected two lines: {opened:?}"
    );

    rig.feed(&[
        PointerEvent::Down(points(price_point.0, price_point.1)),
        PointerEvent::Up(points(price_point.0, price_point.1)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 1);
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "list_hover nil pressed nil")
    );

    // The text-only moonstone line has a passive key in this tooltip-enabled list but no tooltip source.
    let moonstone_y = price_point.1 + 3.0 * 26.0;
    let before = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(price_point.0, moonstone_y))]);
    rig.deliver_fixture_frame_at(1800, 900, 1);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 1);
    }
    let after = notes(&rig);
    assert!(
        !after[before..]
            .iter()
            .any(|line| line.starts_with("tooltip_text ")),
        "the unlisted good line has no tooltip: {:?}",
        &after[before..]
    );

    rig.deliver_fixture_frame_at(1600, 900, 1);
    rig.opened();
    let resized = notes(&rig);
    assert!(resized.iter().any(|line| line == "old_key_live false"));
    let point = resized
        .iter()
        .find(|line| line.starts_with("resized_oats_point "))
        .unwrap();
    let mut coords = point
        .split_whitespace()
        .skip(1)
        .map(|value| value.parse::<f64>().unwrap());
    let resized_point = (coords.next().unwrap(), coords.next().unwrap());
    rig.feed(&[PointerEvent::Move(points(resized_point.0, resized_point.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1600, 900, 1);
    }
    let final_notes = notes(&rig);
    assert!(final_notes.iter().any(|line| line == "tooltip_text oats"));
    assert!(
        final_notes
            .windows(spans.len())
            .any(|window| window == spans),
        "resized price tooltip did not preserve its two lines: {final_notes:?}"
    );
}

#[test]
fn herd_counts_use_full_colour_icons_on_cards_and_open_rows() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)",
        "look_buttons.rows.on_click(); card_object.items[2].on_click()",
    ]);
    rig.opened();
    let card_icons = notes(&rig);
    for (name, icon_x, count_x, colour, count) in [
        ("cattle", 10, 32, "#9a927e", "27/36"),
        ("sheep", 112, 134, "#9a927e", "10/20"),
        ("pigs", 214, 236, "#ece3cb", "9/9"),
    ] {
        assert!(
            card_icons.iter().any(|line| {
                line == &format!("goodsquad ui/goods/{name}.png {icon_x} 76 18 18 -")
            }),
            "card herd {name} icon is at ({},38) UI pixels, untinted 9x9: {card_icons:?}",
            icon_x / 2
        );
        assert!(
            card_text_with_colour_at(&card_icons, count_x, 74, colour, count),
            "card herd {name} count is at ({},37) UI pixels with {colour}: {card_icons:?}",
            count_x / 2
        );
    }
    rig.opened();
    let row_icons = notes(&rig);
    for (name, icon_y, count_y, count) in [
        ("cattle", 104, 102, "27 / 36"),
        ("sheep", 128, 126, "10 / 20"),
        ("pigs", 152, 150, "9 / 9"),
    ] {
        assert!(
            row_icons.iter().any(|line| {
                line == &format!("goodsquad ui/goods/{name}.png 32 {icon_y} 18 18 -")
            }),
            "open row herd {name} icon is at x=16 and one pixel below its text: {row_icons:?}"
        );
        assert!(
            card_text_with_colour_at(&row_icons, 56, count_y, "#ece3cb", count),
            "open row herd {name} count is at x=28: {row_icons:?}"
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
note(string.format("gap_point %g %g", left + 30 * P, top + 42 * P))
local content, part = building.tooltip(cattle_x + 4, 42)
note("card_cattle_source " .. table.concat(content, "|") .. " part=" .. tostring(part))
content, part = building.tooltip(pigs_x + 4, 42)
note("card_pigs_source " .. table.concat(content, "|") .. " part=" .. tostring(part))"#]);
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
            .any(|line| line == "card_cattle_source cattle part=cattle"),
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
            .any(|line| line == "card_pigs_source pigs part=pigs"),
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
        "space between herd icons has no tooltip"
    );
}

#[test]
fn plain_pop_card_tooltip_dispatch_stays_silent() {
    let mut rig = rig(&[
        r#"time(1, 1); select_place("cards", "Cards"); tabs_by_title.Cards:select("pops", true)
local text = require("lib.text"); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tooltips then note("tooltip_text " .. value) end end
capture_tooltips = true
local P = require("lib.ui").P
local left, top = card_object.origin_x + card_object.x, card_object.origin_y + card_object.y + card_object.tops[1]
note(string.format("plain_pop_point %g %g", left + 40 * P, top + 30 * P))"#,
    ]);
    rig.opened();
    let pop = point_note(&rig, "plain_pop_point ");
    assert!(
        rig.key_at(pop.0, pop.1).is_some(),
        "the pop card has a passive hit key"
    );
    let before_pop = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(pop.0, pop.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        !notes(&rig)[before_pop..]
            .iter()
            .any(|line| line.starts_with("tooltip_text ")),
        "a pop card without a tooltip remains silent and does not error"
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
note("row_pigs_source " .. table.concat(content, "|") .. " part=" .. tostring(part))
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
            .any(|line| line == "row_pigs_source pigs part=pigs"),
        "the open-row provider returns pigs with the pigs part"
    );
}

#[test]
fn the_location_tab_stays_text_while_pops_and_buildings_start_as_cards_and_share_the_rows_choice() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling')",
        "announce_tabs()",
        "announce_look()",
        "note('theme track ' .. require('lib.theme').colour('track')); note('theme band ' .. require('lib.theme').colour('band'))",
        "card_object:set_pos(26); note('look_scroll_before ' .. card_object:scroll_position())",
        "note('look_scroll_after_switch ' .. card_object:scroll_position())",
        "select_place('nobody', 'No Economy'); announce_active_panel()",
        "announce_selector_liveness()",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    assert_eq!(
        card_stacks(&rig).last().unwrap(),
        &format!("gap=3 heights={}", vec!["61"; 60].join(","))
    );
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("look cards ") && line.ends_with("selected=true"))
    );
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "theme track #3a3326"));
    assert!(notes(&rig).iter().any(|line| line == "theme band #221c13"));
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "look_scroll_before 26")
    );
    click_look(&mut rig, "rows");
    assert_eq!(
        card_stacks(&rig).last().unwrap(),
        &format!("gap=1 heights={}", vec!["13"; 61].join(","))
    );
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "look_scroll_after_switch 0"),
        "switching looks resets scroll: {:?}",
        notes(&rig)
    );
    click_tab(&mut rig, "buildings");
    assert!(
        card_stacks(&rig)
            .last()
            .unwrap()
            .starts_with("gap=1 heights=")
    );
    rig.opened();
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13"),
        "another place keeps rows"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("active_panel ") && line.contains(" buildings "))
    );
    assert!(
        notes(&rig).iter().any(|line| line.ends_with("nobody@1")),
        "the new place was selected: {:?}",
        notes(&rig)
    );
    click_tab(&mut rig, "location");
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "selector_live false"),
        "location tab destroys selector buttons: {:?}",
        notes(&rig)
    );
    assert_eq!(
        rows(&rig).last().map(String::as_str),
        Some(
            "people: 0|  workers: 0|  dependents: 0|housing: basic 0/0, makeshift 0/0|crowding: 0%|food: 0 days|building land: 0 / 0 m²|storage: 0 / 0 L|lost to overflow last month: 0|prices (base):"
        )
    );
}

fn cards_rendered_fixture() -> Rig {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=61,61")
    );
    click_tab(&mut rig, "buildings");
    rig
}

fn card_text_at(recorded: &[String], x: u32, y: u32, value: &str) -> bool {
    recorded
        .iter()
        .any(|line| line.starts_with(&format!("cardtext {x} {y} ")) && line.ends_with(value))
}

fn card_text_with_colour_at(
    recorded: &[String],
    x: u32,
    y: u32,
    colour: &str,
    value: &str,
) -> bool {
    let suffix = format!(" {colour} {value}");
    recorded
        .iter()
        .any(|line| line.starts_with(&format!("cardtext {x} {y} ")) && line.ends_with(&suffix))
}

fn card_quad_at(recorded: &[String], x: u32, y: u32, width: u32, colour: &str) -> bool {
    recorded.iter().any(|line| {
        line.starts_with("cardquad ") && line.contains(&format!(" {x} {y} {width} 6 {colour}"))
    })
}

#[test]
fn pop_cards_render_mockup_text_positions_and_mood() {
    let recorded = notes(&cards_rendered_fixture());
    for (x, y, value) in [
        (34, 6, "peasant"),
        (34, 28, "ulaid · christianity"),
        (10, 50, "food"),
        (146, 50, "74d"),
        (210, 50, "job"),
        (10, 74, "needs"),
        (146, 74, "mood"),
        (202, 74, "+12"),
        (10, 96, "housing"),
        (94, 96, "380 + 32"),
        (210, 96, "crowd"),
        (270, 96, "95%"),
    ] {
        assert!(
            card_text_at(&recorded, x, y, value),
            "{value} at ({x},{y}): {recorded:?}"
        );
    }
    assert!(card_text_at(&recorded, 270, 224, "—"));
    assert!(recorded.iter().any(|line| line.contains("#8fc06a +12")));
    assert!(recorded.iter().any(|line| line.contains("108w 72d ^")));
}

#[test]
fn pop_card_bars_cover_ratios_clamps_and_empty_jobs() {
    let recorded = notes(&cards_rendered_fixture());
    for (x, y, width) in [(58, 60, 48), (246, 60, 70), (70, 84, 20), (94, 84, 12)] {
        assert!(
            card_quad_at(&recorded, x, y, width, "#ece3cb"),
            "fill at ({x},{y}) width {width}: {recorded:?}"
        );
    }
    assert!(
        card_quad_at(&recorded, 58, 188, 80, "#ece3cb"),
        "300 days clamps to 40px"
    );
    assert!(
        card_quad_at(&recorded, 70, 212, 20, "#ece3cb"),
        "1200 needs clamps to 10px"
    );
    assert!(
        card_quad_at(&recorded, 246, 188, 80, "#3a3326"),
        "zero workers leaves only the job track"
    );
    assert!(
        !card_quad_at(&recorded, 246, 188, 0, "#ece3cb"),
        "zero job emits no fill quad"
    );
    assert!(card_text_at(&recorded, 146, 178, "300d"));
}

#[test]
fn building_cards_render_mockup_values_and_empty_or_clamped_bars() {
    let rig = cards_rendered_fixture();
    let recorded = notes(&rig);
    assert_eq!(
        card_stacks(&rig).get(1).map(String::as_str),
        Some("gap=3 heights=50,39,39,39,39")
    );
    for text in [
        "mixed_farming 8, coastal 0",
        "wage 1.234",
        "housing 3",
        "80/88",
    ] {
        assert!(
            recorded.iter().any(|line| line.contains(text)),
            "missing {text}: {recorded:?}"
        );
    }
    assert!(card_text_at(&recorded, 32, 74, "27/36"));
    assert!(recorded.iter().any(|line| {
        line.starts_with("goodsquad ui/goods/cattle.png ") && line.ends_with(" 18 18 -")
    }));
    assert!(
        card_quad_at(&recorded, 66, 60, 72, "#ece3cb"),
        "farm staff fill is 36px"
    );
    assert!(
        card_quad_at(&recorded, 290, 60, 48, "#ece3cb"),
        "6400/8000 store fill is 24px"
    );
    assert!(
        card_quad_at(&recorded, 66, 166, 80, "#3a3326"),
        "empty staff track"
    );
    assert!(
        card_quad_at(&recorded, 290, 166, 60, "#3a3326"),
        "zero-capacity store track"
    );
    assert!(card_text_at(&recorded, 34, 218, "storage"));
    assert!(
        card_quad_at(&recorded, 290, 334, 60, "#ece3cb"),
        "over-capacity store clamps to 30px"
    );
    assert!(
        card_quad_at(&recorded, 290, 418, 60, "#3a3326"),
        "zero of 8000 draws the track"
    );
    assert!(
        !card_quad_at(&recorded, 290, 418, 0, "#ece3cb"),
        "zero fill creates no quad"
    );
}

#[test]
fn row_items_expand_exclusively_keep_open_rows_across_revisions_and_scroll_clamps() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "announce_look()",
        "time(12, 2)",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,13")
    );
    // The content origin is x=6, y=82 points. The first data row starts 14 UI pixels below it.
    click_panel_point(&mut rig, 40.0, 123.0);
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,100,13")
    );
    assert!(
        notes(&rig).iter().any(|line| line.contains("#4fb3c8")),
        "the open row uses the place outline band: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("#15110a peasant ulaid")),
        "the open row uses title-text ink: {:?}",
        notes(&rig)
    );
    // Once the first row expands, the second row begins at 115 UI pixels below the content origin.
    click_panel_point(&mut rig, 40.0, 325.0);
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,100")
    );
    rig.opened();
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,100")
    );
    assert!(
        rig.sink.quads().iter().any(|quad| quad.clip.is_some()),
        "rows are clipped by their content viewport"
    );
}

#[test]
fn opening_and_closing_a_row_keeps_the_scroll_offset() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling')",
        "announce_tabs()",
        "announce_look()",
        "card_object:set_pos(26); note('row_scroll_before ' .. card_object:scroll_position())",
        "card_object.items[2].on_click(); note('row_scroll_after_open ' .. card_object:scroll_position())",
        "card_object.items[2].on_click(); note('row_scroll_after_close ' .. card_object:scroll_position())",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_scroll_before 26")
    );

    // Invoke the same row callback as a hit after scrolling one line; pointer dispatch is covered above.
    rig.opened();
    assert!(
        card_stacks(&rig)
            .last()
            .unwrap()
            .starts_with("gap=1 heights=13,100,"),
        "first row expanded: {:?}",
        card_stacks(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_scroll_after_open 26"),
        "opening retains the offset: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("cardquad ") && line.ends_with("#4fb3c8")),
        "open row uses place outline: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("#15110a peasant ulaid")),
        "open row title uses title-text: {:?}",
        notes(&rig)
    );
    assert!(
        rig.sink.quads().iter().any(|quad| quad.clip.is_some()),
        "the expanded rows remain clipped to the content viewport"
    );

    // The second click invokes the refreshed row callback and closes it again.
    rig.opened();
    assert!(
        card_stacks(&rig)
            .last()
            .unwrap()
            .starts_with("gap=1 heights=13,13,13,"),
        "second click closes the row: {:?}",
        card_stacks(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_scroll_after_close 26"),
        "closing retains the offset: {:?}",
        notes(&rig)
    );
}

#[test]
fn card_scroll_uses_thirteen_pixel_lines_and_shrinking_clamps_the_offset() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "local many = {}; for i = 1, 40 do many[i] = card_object.items[1] end; \
         card_object:set_items(many, 3); note('scroll0 ' .. card_object:scroll_position()); \
         card_object:scroll({amount = -1, unit = 'lines'}); note('scroll1 ' .. card_object:scroll_position()); \
         card_object:set_items({card_object.items[1]}, 3); note('scroll2 ' .. card_object:scroll_position())",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    // The third UI step reuses the same fixture card to exercise scrolling and a shrinking stack.
    let recorded = notes(&rig);
    assert!(recorded.contains(&"scroll0 0".to_owned()));
    assert!(recorded.contains(&"scroll1 26".to_owned()));
    assert!(recorded.contains(&"scroll2 0".to_owned()));
    assert!(
        rig.sink.quads().iter().any(|quad| quad.clip.is_some()),
        "the stack is clipped to the content viewport"
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
    assert_eq!(
        card_stacks(&rig),
        ["gap=3 heights=13".to_owned(), "gap=3 heights=13".to_owned()]
    );
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
    assert!(
        card_stacks(&rig).is_empty(),
        "a selection without a view creates no list yet"
    );
    rig.opened();
    assert_eq!(notes(&rig).last().map(String::as_str), Some("calls 0"));
    rig.opened();
    assert_eq!(rows(&rig).last().map(String::as_str), Some(LOCATION_ROWS));
    assert!(
        card_stacks(&rig).is_empty(),
        "the location tab stays a text list"
    );
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
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=13")
    );
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
fn card_content_is_clipped_without_wrapping_long_place_values() {
    let mut rig = Rig::new_fixture_body(&script(&[
        "time(1, 1) select_place('fada', 'Fada')",
        "announce_tabs()",
    ]));
    // 300 points leaves a narrow card; long values are clipped by the group's viewport and do not wrap.
    rig.deliver_fixture_frame_at(300, 600, 2);
    rig.opened();
    rig.opened();
    click_tab_at(&mut rig, "pops", (300, 600));
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=61,61")
    );
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

/// The last announced rectangle `[x, y, width, height]`, in whole window points, of the button `key`.
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

fn point_value(value: i64) -> f64 {
    f64::from(i32::try_from(value).expect("fixture window coordinates fit in i32"))
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
fn the_closed_ledger_bar_has_two_icons_at_top_right_and_follows_a_resize() {
    let mut rig = rig(&["time(1, 1) announce_bars()", "announce_bars()"]);
    rig.opened();
    // The two 13 by 15 buttons sit 3 pixels apart inside the 39 by 19 bar, which is 8 points from the top and right edges.
    let check = |rig: &Rig, width: i64| {
        let locations = bar_button(rig, "location");
        let pops = bar_button(rig, "pops");
        assert_eq!(locations[1], 8 + 2 * 2, "the top of the locations button");
        assert_eq!([locations[2], locations[3]], [13 * 2, 15 * 2]);
        assert_eq!(pops[0], locations[0] + (13 + 3) * 2, "the 3-pixel gap");
        assert_eq!([pops[2], pops[3]], [13 * 2, 15 * 2]);
        assert!(
            (pops[0] + pops[2] + 5 * 2 + 8 - width).abs() <= 2,
            "the right edge of the ledger bar: {pops:?} in {width}"
        );
        assert!(
            bar_button(rig, "stockpile")[1] > locations[1] + locations[3],
            "the mode bar is below the ledger bar, not beside it"
        );
    };
    check(&rig, 1800);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "places_text_buttons 0"),
        "the old text button is retired"
    );
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

fn press_bar(rig: &mut Rig, key: &str) {
    let rect = bar_button(rig, key);
    let point = (
        point_value(rect[0]) + point_value(rect[2]) / 2.0,
        point_value(rect[1]) + point_value(rect[3]) / 2.0,
    );
    rig.feed(&[
        PointerEvent::Down(points(point.0, point.1)),
        PointerEvent::Up(points(point.0, point.1)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

fn ledger_origin(rig: &Rig) -> (f64, f64) {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger_origin "))
        .expect("the ledger content origin");
    let mut values = line["ledger_origin ".len()..].split_whitespace();
    (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    )
}

fn click_ledger_row(rig: &mut Rig, row: usize, width: u32, height: u32) {
    const P: f64 = 2.0;
    let (x, y) = ledger_origin(rig);
    let row = f64::from(u32::try_from(row).expect("fixture rows fit in u32"));
    let point = (x + 20.0, y + (row - 0.5) * 13.0 * P);
    let key = rig
        .key_at(point.0, point.1)
        .expect("a published ledger row hit");
    assert!(key.contains("_row_"), "{key}");
    rig.feed(&[
        PointerEvent::Down(points(point.0, point.1)),
        PointerEvent::Up(points(point.0, point.1)),
    ]);
    rig.deliver_fixture_frame_at(width, height, 2);
}

fn ledger_close_point(rig: &Rig) -> (f64, f64) {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger_close "))
        .expect("the ledger close button");
    let mut values = line["ledger_close ".len()..].split_whitespace();
    (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    )
}

#[test]
fn the_ledger_opens_on_each_icon_docks_right_and_close_or_escape_restores_the_bar() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_tabs(); announce_ledger()",
        "announce_ledger()",
        "announce_ledger()",
    ]);
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("ledger 1200 8 600 892")
                && line.contains("draggable=false bar_visible=false tab=pops")),
        "{:?}",
        notes(&rig)
    );
    let pops = rows(&rig).last().unwrap().clone();
    assert!(pops.contains("Rowan@228:left:-"), "{pops}");
    let (x, y) = ledger_close_point(&rig);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "ledger closed bar_visible=true"),
        "{:?}",
        notes(&rig)
    );

    press_bar(&mut rig, "location");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("tab=locations")),
        "{:?}",
        notes(&rig)
    );
    assert!(rig.escape(), "Escape closes the topmost ledger");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "ledger closed bar_visible=true"),
        "{:?}",
        notes(&rig)
    );
}

#[test]
fn ledger_rows_use_revision_order_colours_columns_unicode_clipping_and_select_the_place_tab() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_ledger()",
        "announce_active_panel()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let initial = rows(&rig).last().unwrap().clone();
    assert!(
        initial.contains("name@1:left:#9a927e,people@257:right:#9a927e,food@291:right:#9a927e"),
        "{initial}"
    );
    assert!(
        initial.contains("Rowan@1:left:-,120@257:right:-,215d@291:right:-"),
        "{initial}"
    );
    assert!(
        initial.contains("No Economy@1:left:-;stripe=-;icon=-"),
        "{initial}"
    );
    assert!(initial.contains("Cluain Mhic Nóis@1:left:-"), "{initial}");
    // At 1278 points wide, the right-third outer width is 426 points and its content is 207 UI pixels.
    rig.deliver_fixture_frame_at(1278, 900, 2);
    rig.opened();
    let narrow = rows(&rig).last().unwrap().clone();
    assert!(
        narrow.contains("people@170:right:#9a927e,food@204:right:#9a927e"),
        "{narrow}"
    );
    assert!(
        narrow.contains("\u{D3}\u{D3}\u{D3}..@1:left:-"),
        "the long name clips after whole code points: {narrow}"
    );
    assert!(
        narrow.contains("Cluain Mhic Nóis@1:left:-"),
        "the full Irish place name fits at the 207-unit content width: {narrow}"
    );

    // Select the visible Rowan row. Its immutable view is revision 1, and the ledger remains open beside the place panel.
    click_ledger_row(&mut rig, 2, 1278, 900);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "active_panel Rowan location ledger_closed=false"),
        "{:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("place_calls ") && line.contains("rowan@1")),
        "{:?}",
        notes(&rig)
    );
}

#[test]
fn refreshed_ledger_rows_keep_their_place_mapping_after_inserts_shifts_and_removals() {
    let mut rig = rig(&[
        "time(0, 1) announce_bars()",
        "place_data = {{id='before', name='Aardvark', kind='town', size=1}, \
         {id='rowan', name='Rowan', kind='town', size=3}, \
         {id='fada', name='Cluain Mhic Nóis', kind='monastery', size=2}, \
         {id='long', name=string.rep('Ó', 40), kind='town', size=1}}; time(1, 2); announce_ledger()",
        "place_calls = {}",
        "announce_last_place_call()",
        "place_calls = {}",
        "announce_last_place_call()",
        "place_calls = {}",
        "announce_last_place_call()",
        "place_data = {{id='rowan', name='Rowan', kind='town', size=3}, \
         {id='long', name='Ó', kind='town', size=1}}; time(2, 1); announce_ledger()",
        "place_calls = {}",
        "announce_last_place_call()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    assert!(rows(&rig).last().unwrap().contains("Aardvark@1:left"));

    // The inserted first place, the shifted second place, and the last place must use this revision's row map.
    rig.opened();
    click_ledger_row(&mut rig, 2, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call before@2")
    );
    rig.opened();
    click_ledger_row(&mut rig, 3, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call rowan@2")
    );
    rig.opened();
    click_ledger_row(&mut rig, 5, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call long@2")
    );

    // Removing rows also replaces their mapping; the new last row is long, not the stale fada entry.
    rig.opened();
    rig.opened();
    click_ledger_row(&mut rig, 3, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call long@1")
    );
}

#[test]
fn a_ledger_filled_after_an_empty_first_revision_gets_a_current_row_mapping() {
    let mut rig = rig(&[
        "place_data = {}; time(0, 1) announce_bars()",
        "place_data = {{id='before', name='Aardvark', kind='town', size=1}}; time(1, 2); announce_ledger()",
        "place_calls = {}",
        "announce_last_place_call()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    assert_eq!(
        rows(&rig).last().unwrap().split('|').count(),
        1,
        "only the header is present initially"
    );
    assert!(rows(&rig).last().unwrap().starts_with("name@"));

    rig.opened();
    rig.opened();
    click_ledger_row(&mut rig, 2, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call before@2")
    );
}

#[test]
fn a_pops_ledger_row_selects_pops_and_updates_the_open_location_tab() {
    for previous_tab in ["location", "buildings"] {
        let mut rig = rig(&[
            "time(1, 1) select_place('rowan', 'Rowan') announce_bars(); announce_place_tabs()",
            "announce_place_tabs()",
            "announce_ledger_tabs(); announce_ledger()",
            "announce_place_tabs()",
            "announce_place_tabs()",
            "select_place('fada', 'Fada'); announce_place_tabs()",
        ]);
        rig.opened();
        if previous_tab == "buildings" {
            click_tab(&mut rig, "buildings");
        }
        rig.opened();

        press_bar(&mut rig, "location");
        press_bar(&mut rig, "pops");
        rig.opened();
        click_ledger_row(&mut rig, 1, 1800, 900);
        rig.opened();
        let current = notes(&rig);
        assert!(
            current.iter().any(|line| line == "place_selected pops"),
            "{current:?}"
        );
        assert!(
            current.iter().any(|line| line == "place_cards_open true"),
            "{current:?}"
        );
        assert!(
            current.iter().any(|line| line.contains(" peasant")),
            "Pops content was not drawn: {current:?}"
        );
        assert!(
            current
                .iter()
                .any(|line| line.starts_with("tab pops ") && line.ends_with("selected=true")),
            "Pops tab is not selected: {current:?}"
        );

        click_tab(&mut rig, previous_tab);
        rig.opened();
        let current = notes(&rig);
        assert!(
            current
                .iter()
                .any(|line| line == &format!("place_selected {previous_tab}")),
            "{current:?}"
        );
        if previous_tab == "location" {
            assert!(
                current.iter().any(|line| line == "place_cards_open false"),
                "{current:?}"
            );
        } else {
            assert!(
                current.iter().any(|line| line == "place_cards_open true"),
                "{current:?}"
            );
        }

        rig.opened();
        let current = notes(&rig);
        assert!(
            current
                .iter()
                .any(|line| line == &format!("place_selected {previous_tab}")),
            "show without tab changed it: {current:?}"
        );
    }
}

#[test]
fn startup_viewport_uses_the_window_open_size_before_first_layout_and_resize() {
    let main_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let main = std::fs::read_to_string(main_path).expect("the production UI script");
    let body = format!(
        r#"
{PREFIX}
local real_open = window.open
window.open = function(spec)
  local width, height = real_open(spec)
  opened_width, opened_height = width, height
  return width, height
end
{main}
time(0, 1)
note(string.format("open_size %d %d", opened_width, opened_height))
local windows = require("lib.window")
local probe = windows.new{{title="viewport probe", content_width=100 * P, content_height=50 * P}}
local x, y = probe:position()
local width, height = probe:size()
local expected_x = require("lib.ui").snap((opened_width - width) / 2)
local expected_y = require("lib.ui").snap((opened_height - height) / 2)
note(string.format("viewport_center %g %g %g %g", x, y, expected_x, expected_y))
local visible = math.min(16 * P, width)
local max_x = math.max(visible - width, math.floor((opened_width - visible) / P) * P)
local max_y = math.max(0, math.floor((opened_height - probe.title_height) / P) * P)
probe:move_to(1000000, 1000000)
x, y = probe:position()
note(string.format("viewport_drag %g %g %g %g", x, y, max_x, max_y))
window.on_opened(function()
  if window_objects.ledger ~= nil and not window_objects.ledger.closed then announce_ledger()
  else announce_bars() end
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&body);
    let open = notes(&rig)
        .into_iter()
        .find(|line| line.starts_with("open_size "))
        .expect("window.open's resolved size");
    let mut fields = open.split_whitespace().skip(1);
    let width: u32 = fields.next().unwrap().parse().unwrap();
    let height: u32 = fields.next().unwrap().parse().unwrap();

    // This is the first frame, exactly the size returned from window.open: no resize callback can repair a stale viewport.
    rig.deliver_fixture_frame_at(width, height, 1);
    rig.opened();
    for prefix in ["viewport_center ", "viewport_drag "] {
        let line = notes(&rig)
            .into_iter()
            .find(|line| line.starts_with(prefix))
            .unwrap();
        let values: Vec<f64> = line[prefix.len()..]
            .split_whitespace()
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(&values[..2], &values[2..], "{line}");
    }

    let button = bar_button(&rig, "location");
    let point = (
        point_value(button[0] + button[2] / 2),
        point_value(button[1] + button[3] / 2),
    );
    rig.feed(&[
        PointerEvent::Down(points(point.0, point.1)),
        PointerEvent::Up(points(point.0, point.1)),
    ]);
    rig.deliver_fixture_frame_at(width, height, 1);
    rig.opened();
    let opened = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the ledger geometry at the opened size");
    let mut geometry = opened.split_whitespace();
    assert_eq!(geometry.next(), Some("ledger"));
    let x: f64 = geometry.next().unwrap().parse().unwrap();
    let _y: f64 = geometry.next().unwrap().parse().unwrap();
    let panel_width: f64 = geometry.next().unwrap().parse().unwrap();
    assert!(
        (x + panel_width - f64::from(width)).abs() < f64::EPSILON,
        "{opened}"
    );

    rig.deliver_fixture_frame_at(width + 200, height + 100, 1);
    rig.opened();
    let resized = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the ledger geometry after resize");
    let mut geometry = resized.split_whitespace();
    let _ = geometry.next();
    let x: f64 = geometry.next().unwrap().parse().unwrap();
    let _y: f64 = geometry.next().unwrap().parse().unwrap();
    let panel_width: f64 = geometry.next().unwrap().parse().unwrap();
    assert!(
        (x + panel_width - f64::from(width + 200)).abs() < f64::EPSILON,
        "{resized}"
    );
}

#[test]
fn card_scrolling_keeps_fractional_points_and_snaps_only_the_drawn_offset() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling')",
        "announce_tabs()",
        "card_object:set_pos(26); local start_y = card_object.shown_y; \
         card_object:scroll({amount=1, unit='points'}); card_object:scroll({amount=1, unit='points'}); \
         note('scroll_down ' .. card_object:scroll_position() .. ' ' .. card_object.shown_y - start_y); \
         card_object:scroll({amount=-1, unit='points'}); card_object:scroll({amount=-1, unit='points'}); \
         note('scroll_up ' .. card_object:scroll_position() .. ' ' .. card_object.shown_y - start_y); \
         card_object:scroll({amount=-0.5, unit='points'}); note('fractional ' .. card_object:scroll_position()); \
         card_object:scroll({amount=0.25, unit='points'}); note('fractional_2 ' .. card_object:scroll_position()); \
         card_object:set_pos(26); card_object:scroll({amount=-1, unit='lines'}); note('line_scroll ' .. card_object:scroll_position()); \
         card_object:set_items({card_object.items[1]}, 3); note('shrink_scroll ' .. card_object:scroll_position()); \
         local key = next(card_object.item_keys); note('card_grid ' .. key .. ' ' .. card_object.shown_x .. ' ' .. card_object.shown_y)",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded.iter().any(|line| line == "scroll_down 24 2"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "scroll_up 26 0"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "fractional 26.5"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "fractional_2 26.25"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "line_scroll 52"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "shrink_scroll 0"),
        "{recorded:?}"
    );

    let grid = recorded
        .iter()
        .find(|line| line.starts_with("card_grid "))
        .expect("card hit geometry");
    let mut fields = grid.split_whitespace();
    let _ = fields.next();
    let key = fields.next().unwrap();
    let x: f64 = fields.next().unwrap().parse().unwrap();
    let y: f64 = fields.next().unwrap().parse().unwrap();
    assert!(x.rem_euclid(2.0).abs() < f64::EPSILON, "{grid}");
    assert!(y.rem_euclid(2.0).abs() < f64::EPSILON, "{grid}");
    assert_eq!(rig.key_at(x + 2.0, y + 2.0).as_deref(), Some(key));
}

#[test]
fn pop_rows_have_type_tints_and_route_to_the_place_pops_tab() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_tabs()",
        "announce_active_panel()",
    ]);
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let items = rows(&rig).last().unwrap().clone();
    assert!(items.contains("peasant ulaid@16:left:#c9932e,120@224:right:-,Rowan@228:left:-;stripe=#c9932e;icon=pops"), "{items}");
    assert!(
        items.contains("#ece3cb"),
        "unknown pop type falls back to text: {items}"
    );
    assert!(items.contains("monk mide@16:left:#4fb3c8"), "{items}");
    // The first pop belongs to Rowan; selecting it opens the location panel on its Pops tab.
    click_ledger_row(&mut rig, 1, 1800, 900);
    rig.opened();
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "active_panel Rowan pops ledger_closed=false"),
        "{:?}",
        notes(&rig)
    );
}

#[test]
fn the_pops_ledger_clips_inside_its_column_and_tracks_the_1200_by_700_resize() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_pop_clip(); announce_ledger()",
        "announce_pop_clip(); announce_ledger()",
    ]);
    rig.opened();
    press_bar(&mut rig, "pops");

    // A 1278-point view makes the right-third ledger 426 points wide and its clipped row column 87 UI pixels wide.
    rig.deliver_fixture_frame_at(1278, 900, 2);
    rig.opened();
    let clip = notes(&rig)
        .into_iter()
        .find(|line| line.starts_with("pop_clip "))
        .expect("the long pop label's rendered extent");
    let field = |name: &str| -> u32 {
        clip.split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("missing {name}: {clip}"))
            .parse()
            .unwrap_or_else(|_| panic!("bad {name}: {clip}"))
    };
    assert_eq!(
        field("x"),
        16,
        "the label starts at its specified column: {clip}"
    );
    assert!(
        field("right") < 103,
        "the clipped glyphs end before x=103: {clip}"
    );

    // At 1200x700 points the ledger remains the right third: 400 points wide after the resize.
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    let ledger = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the resized ledger geometry");
    let width = ledger
        .split_whitespace()
        .nth(3)
        .expect("ledger width")
        .parse::<u32>()
        .expect("numeric ledger width");
    assert_eq!(
        width, 400,
        "the ledger follows the resized right third: {ledger}"
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
