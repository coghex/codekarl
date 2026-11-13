//! The location panel (milestone 9, slice 3) in a real UI VM: the shipped `scripts/ui/main.lua` run over a stub `game` whose
//! `game.place` serves fixture views, with the panel's list rows recorded as the lists are created and updated.
//!
//! A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use std::{collections::BTreeMap, fmt::Write, sync::Arc};

use ck_lua::{ConfigValue, HostFunction};
use ck_window::{PointerEvent, ScrollUnit};

use super::group_tests::Rig;
use super::widget_witness_tests::points;

fn scroll_lines(x: f64, y: f64, amount: i32) -> PointerEvent {
    PointerEvent::Scroll {
        position: points(x, y),
        amount: amount * 256,
        unit: ScrollUnit::Lines,
    }
}

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
    land = {usable = {pasture = 8}, land = {pasture = 8, bog = 4}, sea = 2},
    last_month = {closed = false, produced = {}, used_by_pops = {}, lost = {}},
    basic_housing_capacity = 100, basic_housing_occupants = 80,
    makeshift_housing_capacity = 20, makeshift_housing_occupants = 10, crowding_pm = 500,
    prices = {price("oats", 100000, 100), price("ale", 2500, 2)},
    pops = {{kind = "peasant", culture = "ulaid", religion = "christianity", workers = workers, dependents = 40, employed = 70,
      unemployed = workers - 70, food_days = 215, necessities = 0, niceties = 0, luxuries = 0, satisfaction = 0,
      births = births, deaths = 1, basic_housing = 80, makeshift_housing = 10, crowding_pm = 500}},
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
  land = {usable = {}, land = {}, sea = 0}, last_month = {closed = false, produced = {}, used_by_pops = {}, lost = {}},
  basic_housing_capacity = 0, basic_housing_occupants = 0, makeshift_housing_capacity = 0, makeshift_housing_occupants = 0, crowding_pm = 0,
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
    food_days = 74, food = {{good = "oats", units = 300, unit = "bushel", share_pm = 658},
      {good = "barley", units = 100, unit = "bushel", share_pm = 341}},
    necessities = 1000, niceties = 620, luxuries = 80, satisfaction = 12, births = 2, deaths = 1,
    basic_housing = 380, makeshift_housing = 32, crowding_pm = 950},
  {kind = "monk", culture = "mide", religion = "christianity", workers = 0, dependents = 5, employed = 0, unemployed = 0,
    food_days = 300, necessities = 1200, niceties = 0, luxuries = 0, satisfaction = -3, births = 0, deaths = 2,
    basic_housing = 0, makeshift_housing = 0, crowding_pm = false},
}
cards_place.buildings = {
    {kind = "farm", levels = 8, methods = {{id = "mixed_farming", levels = 8}, {id = "coastal_subsistence", levels = 1}}, filled = 80, open = 8,
    herds = {{species = "cattle", head = 27, maximum = 36}, {species = "sheep", head = 10, maximum = 20},
      {species = "pigs", head = 64, maximum = 64}}, store_used_l = 6400, store_capacity_l = 8000, wage = 3200,
    wages = {{month = 5, year = 769, wage_milli = 3200, paid = 18}, {month = 4, wage_milli = 2900},
      {month = 3, year = 769, wage_milli = 2900, paid = 10}, {month = 2, year = 769, wage_milli = 2900, paid = 10},
      {month = 1, year = 769, wage_milli = 2900, paid = 10}, {month = 12, year = 768, wage_milli = 2900, paid = 10},
      {month = 11, year = 768, wage_milli = 2900, paid = 10}, {month = 10, year = 768, wage_milli = 2900, paid = 10},
      {month = 9, year = 768, wage_milli = 2900, paid = 10}, {month = 8, year = 768, wage_milli = 2900, paid = 10},
      {month = 7, year = 768, wage_milli = 2900, paid = 10}, {month = 6, year = 768, wage_milli = 2900, paid = 10}},
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
    if window.actual_place ~= nil then return window.actual_place(id, revision) end
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
capture_tooltip_sources = false
local tooltip = require("lib.tooltip")
local real_tooltip_attach = tooltip.attach
tooltip.attach = function(key, source)
  if type(source) == "function" then
    local original = source
    source = function(...)
      local content, part = original(...)
      if capture_tooltip_sources then
        local lines = {}
        for _, line in ipairs(content or {}) do
          local chunks = {}
          if type(line) == "table" then
            for _, span in ipairs(line) do chunks[#chunks + 1] = type(span) == "table" and span.text or tostring(span) end
          else
            chunks[1] = tostring(line)
          end
          lines[#lines + 1] = table.concat(chunks)
        end
        note("source " .. key .. " " .. table.concat(lines, "|") .. " part=" .. tostring(part or "-"))
      end
      return content, part
    end
  end
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
  if type(content) == "table" then
    if content.title ~= nil then lines[#lines + 1] = "title=" .. content.title end
    for _, line in ipairs(content) do
      lines[#lines + 1] = line_text(line)
    end
  end
  note("tooltip " .. name .. " " .. table.concat(lines, "|") .. " part=" .. tostring(part or "-"))
end
function announce_main_tooltips()
  note_tooltip("date", "date_readout")
  note_tooltip("yield", mode_buttons.yield.key)
  if date_readout_rect ~= nil then
    note(string.format("date_hit %g %g %g %g", date_readout_rect.x, date_readout_rect.y,
      date_readout_rect.width, date_readout_rect.height))
  end
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
colour_watch = false
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
  if name ~= nil and name:match("^ui/theme/cut3_") then note("tooltip_panel_quad " .. name) end
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
  line.ck_test_text = content
  line.ck_test_runs = {{colour = line.colour, text = content}}
end
local real_text_set_segments = text_library.set_segments
text_library.set_segments = function(line, segments)
  real_text_set_segments(line, segments)
  local words, runs = {}, {}
  for _, segment in ipairs(segments) do
    words[#words + 1] = segment.text
    runs[#runs + 1] = {colour = segment.colour or line.colour, text = segment.text}
  end
  line.ck_test_text = table.concat(words)
  line.ck_test_runs = runs
end
local real_text_set_colour = text_library.set_colour
text_library.set_colour = function(line, colour)
  real_text_set_colour(line, colour)
  if line.ck_test_text ~= nil then line.ck_test_runs = {{colour = colour, text = line.ck_test_text}} end
end
local real_text_move = text_library.move
text_library.move = function(line, x, y)
  real_text_move(line, x, y)
  if drawing_card and line.ck_test_text ~= nil then
    note(string.format("cardtext %g %g %d %s %s", line.x, line.y, line.width, line.colour, line.ck_test_text))
    for _, run in ipairs(line.ck_test_runs or {}) do
      note(string.format("cardrun %g %g %s=%s", line.x, line.y, run.colour, run.text))
    end
  end
  if colour_watch and line.ck_test_text ~= nil then
    for _, run in ipairs(line.ck_test_runs or {}) do
      note(string.format("tintline %s|%s=%s", line.ck_test_text, run.colour, run.text))
    end
  end
end
local real_group_visible = render.set_group_visible
local real_set_hit = render.set_hit
date_readout_rect = nil
render.set_hit = function(key, rects, group)
  if key == "places_bar" then bar_group = group end
  if key == "date_readout" and rects ~= nil and rects[1] ~= nil then date_readout_rect = rects[1] end
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
function use_many_ledger_places()
  local source = {
    {id = "rowan", name = "Rowan", kind = "town", size = 3},
    {id = "cards", name = "Cards", kind = "town", size = 3},
    {id = "nobody", name = "No Economy", kind = "town", size = 1},
    {id = "long", name = string.rep("Ó", 40), kind = "town", size = 1},
  }
  place_data = {}
  for index = 1, 48 do place_data[index] = source[(index - 1) % #source + 1] end
end

-- Every list's rows, as created and as replaced.
local lists = require("lib.list")
local real_list_new = lists.new
local function row_text(item)
  if type(item) == "string" then return item end
  local first = item.cells[1] ~= nil and item.cells[1].text or ""
  local location_line = first:match("^people:") or first:match("^  workers:") or first:match("^  dependents:")
    or first:match("^housing:") or first:match("^crowding:") or first:match("^food:") or first:match("^building land:")
    or first:match("^storage:") or first:match("^lost to overflow last month:")
  if item.tooltip ~= nil and item.cells[1] ~= nil and item.cells[1].x == 1 and location_line then
    local joined = {}
    for _, cell in ipairs(item.cells) do joined[#joined + 1] = cell.text end
    return table.concat(joined)
  end
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
function announce_location_tooltips()
  local line = list_object
  local function tip(row, x, name)
    local y = line.origin_y + line.y - line.s + (row - 0.5) * 26
    note_tooltip(name, line.row_keys[row], line.origin_x + line.x + x, y)
  end
  local ui_pixel = require("lib.ui").P
  local people_cells = line.items[1].cells
  tip(1, people_cells[1].x * ui_pixel + font.layout(people_cells[1].text).width * ui_pixel / 2, "people_word")
  tip(1, people_cells[2].x * ui_pixel + 1, "people_number")
  local food_row = line.items[6]
  note("food_row_type " .. tostring(type(food_row)))
  tip(6, food_row.cells[2].x * ui_pixel + 1, "food_number")
  note_tooltip("title", window_objects.Rowan.title_key)
  for row_index, row in ipairs(line.items) do
    if type(row) == "table" and row.tooltip ~= nil then
      local y = line.origin_y + line.y - line.s + (row_index - 0.5) * 26
      for cell_index, cell in ipairs(row.cells) do
        local x = line.origin_x + line.x + cell.x * ui_pixel + font.layout(cell.text).width * ui_pixel / 2
        note_tooltip("tab_" .. row_index .. "_" .. cell_index, line.row_keys[row_index], x, y)
      end
    end
  end
end
function announce_wrapped_housing_source()
  local place = game.place("rowan", 1)
  local module = require("lib.location")
  local rows = module.rows(module.lines("location", place), 204)
  local continued = false
  for _, row in ipairs(rows) do
    if row.tooltip_place == place.id then
      local text = ""
      for _, cell in ipairs(row.cells) do text = text .. cell.text end
      if text:match("^  ") and (text:find("80/100", 1, true) or text:find("10/20", 1, true)) then
        note("housing_row " .. text)
        for index, cell in ipairs(row.cells) do
          if cell.text:find("/", 1, true) then
            local content = row.tooltip(0, 0, index)
            if content ~= nil then
              local parts = {}
              for _, value in ipairs(content) do
                parts[#parts + 1] = type(value) == "table" and value.text or tostring(value)
              end
              note("housing_continuation " .. table.concat(parts, "|"))
              continued = true
              break
            end
          end
        end
      end
    end
  end
  note("housing_continuation_found " .. tostring(continued))
end
function announce_ledger_cell_tips()
  local line = list_object
  local function tip(row, x, name)
    local y = line.origin_y + line.y - line.s + (row - 0.5) * 26
    note_tooltip(name, line.row_keys[row], line.origin_x + line.x + x, y)
  end
  local place = line.items[2]
  tip(1, 2 + font.layout("name").width, "heading_name")
  tip(1, line.width - 84, "heading_people")
  tip(1, line.width - 20, "heading_food")
  tip(2, 3, "place_name")
  tip(2, line.width - 84, "place_people")
  tip(2, line.width - 20, "place_food")
  note("ledger_row_count " .. tostring(#line.items) .. " " .. tostring(place and place.cells[1].text))
  capture_tooltip_sources = true
end
function announce_ledger_pop_tips()
  local line = list_object
  local item = line.items[1]
  local ui_pixel = require("lib.ui").P
  local y = line.origin_y + line.y - line.s + 13 * ui_pixel
  local function tip(x, name)
    note_tooltip(name, line.row_keys[1], line.origin_x + line.x + x, y)
  end
  tip(7 * ui_pixel, "pop_icon")
  tip(item.cells[1].x * ui_pixel + 1, "pop_kind")
  tip(item.cells[2].x * ui_pixel - 1, "pop_size")
  tip(item.cells[3].x * ui_pixel + 1, "pop_place")
  note("pop_size_total " .. item.cells[2].text)
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
    script_with_prelude("", steps)
}

fn script_with_prelude(prelude: &str, steps: &[&str]) -> String {
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
        "{PREFIX}\n{prelude}\n{main}\n{queued}\nwindow.on_opened(function() table.remove(steps, 1)() end)\n"
    )
}

/// A rig at 2 points per pixel, its window set to 1800 by 900 points: the left third is 600 points, wide enough for every fixture
/// line without wrapping.
fn rig(steps: &[&str]) -> Rig {
    let mut rig = Rig::new_fixture_body(&script(steps));
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig
}

fn rig_at(steps: &[&str], width: u32, height: u32, scale: u32) -> Rig {
    let mut rig = Rig::new_fixture_body(&script(steps));
    rig.deliver_fixture_frame_at(width, height, scale);
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

fn tint_line(recorded: &[String], content: &str, colour: &str, fragment: &str) -> bool {
    let wanted = format!("tintline {content}|{colour}={fragment}");
    recorded.iter().any(|line| line == &wanted)
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

const LOCATION_ROWS: &str = "people: 120|  workers: 80|  dependents: 40|housing: basic 80/100, makeshift 10/20|crowding: 50%|food: 215 days|building land: 4200 / 24200 m²|storage: 5400 / 86400 L|lost to overflow last month: 0|prices (base):|100 (100)@16:left:-;stripe=-;icon=goods/oats|2.5 (2)@16:left:-;stripe=-;icon=goods/ale";

#[test]
fn the_time_bar_and_location_tab_split_terms_numbers_and_ordinary_punctuation() {
    let mut rig = Rig::new_fixture_body(&script_with_prelude(
        "colour_watch = true",
        &["time(1, 1) select_place('rowan', 'Rowan')"],
    ));
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        tint_line(&recorded, "1 Jan 768 · 01:00", "#5fb35a", "1"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "1 Jan 768 · 01:00", "#ece3cb", " Jan "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "1 Jan 768 · 01:00", "#ece3cb", " · "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "people: ", "#92ddea", "people: "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "120", "#5fb35a", "120"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "basic ", "#92ddea", "basic "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "80/100", "#5fb35a", "80"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "80/100", "#ece3cb", "/"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "215 days", "#5fb35a", "215"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "215 days", "#ece3cb", " days"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "4200 / 24200 m²", "#ece3cb", " m²"),
        "{recorded:?}"
    );
}

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
    for (name, title) in [
        ("mode_stockpile", "stockpile map"),
        ("mode_yield", "yield map"),
        ("mode_elevation", "elevation map"),
    ] {
        assert!(
            recorded
                .iter()
                .any(|line| line.starts_with(&format!("tooltip {name} title={title}|"))),
            "missing {name} title={title}: {recorded:?}"
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
        ("pigs", 214, 236, "#ece3cb", "64/64"),
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
    assert_eq!(
        card_run_count_at(&card_icons, 236, 74, "#5fb35a", "64"),
        2,
        "both maximum herd digits are green: {card_icons:?}"
    );
    assert!(
        card_run_at(&card_icons, 236, 74, "#ece3cb", "/"),
        "maximum herd separator keeps text colour: {card_icons:?}"
    );
    rig.opened();
    let row_icons = notes(&rig);
    for (name, icon_y, count_y, count) in [
        ("cattle", 104, 102, "27 / 36"),
        ("sheep", 128, 126, "10 / 20"),
        ("pigs", 152, 150, "64 / 64"),
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

const BUILDING_CARD_LAYOUT_PROBE: &str = r##"
local P = require("lib.ui").P
local building = VIEWS[1].cards.buildings[1]
local item = card_object.items[1]
local captured, record = {}, note
note = function(value) captured[#captured + 1] = value; record(value) end
local p, s = font.layout("positions").width, font.layout("storage").width
local function layout(width)
  local count = building.filled .. "/" .. (building.filled + building.open)
  local c = font.layout(count).width
  local fixed = 5 + p + 4 + 4 + c + 12 + s + 4 + 6
  local room = width - fixed
  local out = {step = 6, count = count, count_width = c}
  if room >= 34 then
    out.step, out.positions_x, out.positions_width = 1, 5, p
    out.positions_bar_x, out.positions_bar_width = 5 + p + 4, 24
    out.count_x = out.positions_bar_x + out.positions_bar_width + 4
    out.storage_x = out.count_x + c + 12
    out.storage_bar_x, out.storage_bar_width = out.storage_x + s + 4, room - 24
  elseif room >= 12 then
    out.step, out.positions_x, out.positions_width = 2, 5, p
    out.positions_bar_width = math.floor(room / 2)
    out.positions_bar_x = 5 + p + 4
    out.count_x = out.positions_bar_x + out.positions_bar_width + 4
    out.storage_x = out.count_x + c + 12
    out.storage_bar_x, out.storage_bar_width = out.storage_x + s + 4, room - out.positions_bar_width
  elseif width >= 5 + p + 4 + c + 12 + s + 6 then
    out.step, out.positions_x, out.positions_width = 3, 5, p
    out.count_x, out.storage_x = 5 + p + 4, 5 + p + 4 + c + 12
  elseif width >= 5 + p + 4 + c + 6 then
    out.step, out.positions_x, out.positions_width = 4, 5, p
    out.count_x = 5 + p + 4
  elseif width >= 5 + p + 6 then
    out.step, out.positions_x, out.positions_width = 5, 5, p
  end
  return out
end
local function tip(x, y)
  local content, part = item.tooltip(x, y)
  return content, part
end
local function expect_tip(x, y, expected_title, expected_part)
  local content, part = tip(x, y)
  assert(part == expected_part, "tooltip part at " .. x .. "," .. y .. ": " .. tostring(part))
  if expected_title ~= nil then
    assert(content ~= nil and content.title == expected_title,
      "tooltip title at " .. x .. "," .. y .. ": " .. tostring(content and content.title))
  elseif expected_part == nil then
    assert(content == nil, "unexpected tooltip at " .. x .. "," .. y .. " part=" .. tostring(part)
      .. " title=" .. tostring(content and content.title))
  end
end
local function draw_case(name, width, expected_step, filled, open)
  building.filled, building.open = filled or 80, open or 8
  local expected = layout(width)
  assert(expected.step == expected_step, name .. " expected step " .. expected_step .. " got " .. expected.step)
  local start = #captured + 1
  local draw_ok, drawn = pcall(item.draw, card_object.group, 0, 0, width * P)
  assert(draw_ok, name .. " draw failed: " .. tostring(drawn))
  local actual_text, actual_tracks = {}, {}
  for index = start, #captured do
    local line = captured[index]
    local x, y, line_width, colour, content = line:match("^cardtext ([%d%.]+) ([%d%.]+) (%d+) ([^ ]+) (.*)$")
    if x ~= nil and tonumber(y) == 50 then
      actual_text[#actual_text + 1] = {x = tonumber(x), width = tonumber(line_width), text = content}
      assert(tonumber(x) + tonumber(line_width) <= (width - 6) * P,
        name .. " text exceeds the right margin: " .. line)
    end
    local _, quad_x, quad_y, quad_width, quad_height, tint =
      line:match("^cardquad (%S+) ([%d%.]+) ([%d%.]+) ([%d%.]+) ([%d%.]+) (%S+)$")
    if quad_x ~= nil and tonumber(quad_y) == 60 and tonumber(quad_height) == 6 and tint == "#3a3326" then
      actual_tracks[#actual_tracks + 1] = {x = tonumber(quad_x), width = tonumber(quad_width)}
      assert(tonumber(quad_x) + tonumber(quad_width) <= (width - 6) * P,
        name .. " bar exceeds the right margin: " .. line)
      assert(tonumber(quad_width) >= 6 * P, name .. " bar is narrower than six UI pixels: " .. line)
    end
  end
  local expected_text = {}
  if expected.positions_x ~= nil then expected_text[#expected_text + 1] = {x = expected.positions_x, text = "positions"} end
  if expected.count_x ~= nil then expected_text[#expected_text + 1] = {x = expected.count_x, text = expected.count} end
  if expected.storage_x ~= nil then expected_text[#expected_text + 1] = {x = expected.storage_x, text = "storage"} end
  assert(#actual_text == #expected_text, name .. " third-line text count: " .. #actual_text .. " vs " .. #expected_text)
  for index, wanted in ipairs(expected_text) do
    local got = actual_text[index]
    assert(got.text == wanted.text and got.x == wanted.x * P,
      name .. " third-line text " .. index .. " expected " .. wanted.text .. " at " .. wanted.x * P
        .. ", got " .. got.text .. " at " .. got.x)
    assert(got.width == font.layout(wanted.text).width,
      name .. " uses the measured font width for " .. wanted.text .. ": got " .. got.width
        .. " expected " .. font.layout(wanted.text).width)
  end
  local expected_tracks = {}
  if expected.positions_bar_x ~= nil then
    expected_tracks[#expected_tracks + 1] = {x = expected.positions_bar_x, width = expected.positions_bar_width}
    expected_tracks[#expected_tracks + 1] = {x = expected.storage_bar_x, width = expected.storage_bar_width}
  end
  assert(#actual_tracks == #expected_tracks, name .. " track count: " .. #actual_tracks .. " vs " .. #expected_tracks)
  for index, wanted in ipairs(expected_tracks) do
    local got = actual_tracks[index]
    assert(got.x == wanted.x * P and got.width == wanted.width * P,
      name .. " track " .. index .. " geometry mismatch")
  end

  if expected.positions_x ~= nil then
    expect_tip(expected.positions_x + p // 2, 26, "position", "positions")
  else
    expect_tip(5, 26, nil, nil)
  end
  if expected.positions_bar_x ~= nil then
    expect_tip(expected.positions_bar_x + expected.positions_bar_width // 2, 31, nil, "staff_bar")
  else
    expect_tip(5 + p + 1, 31, nil, nil)
  end
  if expected.count_x ~= nil then
    expect_tip(expected.count_x + 1, 26, nil, "staff_count")
  else
    expect_tip(5 + p + 5, 26, nil, nil)
  end
  if expected.storage_x ~= nil then
    expect_tip(expected.storage_x + 1, 26, "storage", "storage")
  else
    if expected.count_x ~= nil then
      expect_tip(expected.count_x + expected.count_width + 1, 26, nil, nil)
    else
      expect_tip(5 + p + 2, 26, nil, nil)
    end
  end
  if expected.storage_bar_x ~= nil then
    expect_tip(expected.storage_bar_x + 1, 31, nil, "store_bar")
  else
    local after_label
    if expected.storage_x ~= nil then after_label = expected.storage_x + s + 1
    elseif expected.count_x ~= nil then after_label = expected.count_x + expected.count_width + 13
    else after_label = 5 + p + 2 end
    expect_tip(after_label, 31, nil, nil)
  end
  if drawn ~= nil and drawn.destroy ~= nil then drawn:destroy() end
end

local W = card_object.width // P
local ordinary = layout(W)
assert(ordinary.step == 1, "the normal 1280x800 card uses the approved full layout")
draw_case("approved-usual-12-16", W, 1, 12, 4)
draw_case("usual", W, 1)
local F = 5 + p + 4 + 4 + font.layout("80/88").width + 12 + s + 4 + 6
draw_case("full-minimum", F + 34, 1)
draw_case("shared-below-full", F + 33, 2)
draw_case("shared-minimum", F + 12, 2)
draw_case("no-bars", F + 11, 3)
local T4 = 5 + p + 4 + font.layout("80/88").width + 6
draw_case("no-storage", T4, 4)
draw_case("below-no-storage", T4 - 1, 5)
local T5 = 5 + p + 6
draw_case("positions-only", T5, 5)
draw_case("empty", T5 - 1, 6)
draw_case("minimum-empty", 5 + p + 5, 6)
draw_case("long-count", W, 2, 1000, 0)
building.filled, building.open = 80, 8
local min_width = require("lib.location").min_width()
local _, outer_height = panel_window:size()
panel_window:set_outer_size(min_width, outer_height)
local narrow_width = card_object.width // P
local narrow = layout(narrow_width)
draw_case("minimum-window", narrow_width, narrow.step)
note("ck284 width measurements " .. p .. " " .. s .. " " .. font.layout("80/88").width .. " " .. W
  .. " " .. narrow_width .. " " .. min_width)
"##;

#[test]
fn building_card_third_line_uses_real_widths_for_each_fallback_step() {
    let rig = rig_at(
        &[
            "time(1, 1) select_place('cards', 'Cards')",
            "announce_tabs(); tabs_by_title.Cards:select('buildings', true)",
            BUILDING_CARD_LAYOUT_PROBE,
        ],
        1280,
        800,
        2,
    );
    let mut rig = rig;
    rig.opened();
    rig.opened();
    rig.opened();
    let captured = notes(&rig);
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("ck284 width measurements ")),
        "measured font and card widths were recorded: {captured:?}"
    );
}
#[test]
fn building_rows_header_uses_positions() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs(); tabs_by_title.Cards:select('buildings', true)",
        "look_buttons.rows.on_click()",
    ]);
    rig.opened();
    rig.opened();
    rig.opened();
    let captured = notes(&rig);
    assert!(card_run(&captured, "#9a927e", "positions"), "{captured:?}");
    assert!(!card_run(&captured, "#9a927e", "staff"), "{captured:?}");
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

fn cards_rendered_fixture_at(width: u32, height: u32, scale: u32) -> Rig {
    let mut rig = rig_at(
        &[
            "time(1, 1) select_place('cards', 'Cards')",
            "announce_tabs()",
        ],
        width,
        height,
        scale,
    );
    rig.opened();
    rig.opened();
    click_tab_at(&mut rig, "pops", (width, height));
    click_tab_at(&mut rig, "buildings", (width, height));
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

fn card_run(recorded: &[String], colour: &str, value: &str) -> bool {
    let wanted = format!("{colour}={value}");
    recorded.iter().any(|line| {
        line.strip_prefix("cardrun ")
            .is_some_and(|run| run.splitn(3, ' ').nth(2) == Some(wanted.as_str()))
    })
}

fn card_run_at(recorded: &[String], x: u32, y: u32, colour: &str, value: &str) -> bool {
    recorded
        .iter()
        .any(|line| line == &format!("cardrun {x} {y} {colour}={value}"))
}

fn card_run_count_at(recorded: &[String], x: u32, y: u32, colour: &str, value: &str) -> usize {
    let wanted = format!("cardrun {x} {y} {colour}={value}");
    recorded.iter().filter(|line| *line == &wanted).count()
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
    for (colour, value) in [
        ("#92ddea", "peasant"),
        ("#92ddea", "ulaid"),
        ("#92ddea", "christianity"),
        ("#92ddea", "food"),
        ("#5fb35a", "12"),
        ("#5fb35a", "3"),
        ("#ece3cb", "+"),
        ("#ece3cb", "-"),
        ("#9a927e", " · "),
        ("#9a927e", "w "),
    ] {
        assert!(
            card_run(&recorded, colour, value),
            "{value:?} in {colour}: {recorded:?}"
        );
    }
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
    let rig = cards_rendered_fixture_at(1280, 800, 2);
    let recorded = notes(&rig);
    assert_eq!(
        card_stacks(&rig).get(1).map(String::as_str),
        Some("gap=3 heights=50,39,39,39,39")
    );
    for text in [
        "mixed_farming 8, coastal_subsistence 1",
        "wage 3.2",
        "housing 3",
        "80/88",
    ] {
        assert!(
            recorded.iter().any(|line| line.contains(text)),
            "missing {text}: {recorded:?}"
        );
    }
    for (colour, value) in [
        ("#92ddea", "farm"),
        ("#92ddea", "wage "),
        ("#92ddea", "mixed_farming"),
        ("#5fb35a", "3.2"),
        ("#5fb35a", "8"),
        ("#5fb35a", "27"),
        ("#5fb35a", "36"),
        ("#9a927e", "/"),
        ("#92ddea", "positions"),
        ("#92ddea", "storage"),
    ] {
        assert!(
            card_run(&recorded, colour, value),
            "{value:?} in {colour}: {recorded:?}"
        );
    }
    assert!(card_text_at(&recorded, 32, 74, "27/36"));
    assert!(recorded.iter().any(|line| {
        line.starts_with("goodsquad ui/goods/cattle.png ") && line.ends_with(" 18 18 -")
    }));
    assert!(
        card_quad_at(&recorded, 116, 60, 42, "#ece3cb"),
        "the positions bar uses the approved 24px width and fills 21px"
    );
    assert!(
        card_quad_at(&recorded, 346, 60, 44, "#ece3cb"),
        "the storage bar fills proportionally within the remaining width: {recorded:?}"
    );
    assert!(
        card_quad_at(&recorded, 116, 166, 48, "#3a3326"),
        "empty positions track"
    );
    assert!(
        card_quad_at(&recorded, 322, 166, 80, "#3a3326"),
        "zero-capacity storage track: {recorded:?}"
    );
    assert!(card_run(&recorded, "#92ddea", "storage"));
    assert!(
        card_quad_at(&recorded, 322, 334, 80, "#ece3cb"),
        "over-capacity storage fill clamps to its track"
    );
    assert!(
        card_quad_at(&recorded, 322, 418, 80, "#3a3326"),
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
    let recorded = notes(&rig);
    assert!(
        card_run(&recorded, "#663069", "180"),
        "open pop header digits: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#15110a", "+"),
        "open pop header sign: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#92ddea", "ulaid"),
        "open pop culture: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#5fb35a", "108"),
        "open pop detail numbers: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#ece3cb", " · "),
        "open pop row separator keeps text colour: {recorded:?}"
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
fn an_open_storehouse_row_keeps_its_plain_storage_fallback() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "tabs_by_title.Cards:select('buildings', true); look_buttons.rows.on_click(); card_object.items[4].on_click()",
    ]);
    rig.opened();
    rig.opened();
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        card_run(&recorded, "#ece3cb", "storage"),
        "the opened storehouse row retains its plain storage fallback: {recorded:?}"
    );
    assert!(
        card_run_at(&recorded, 32, 110, "#ece3cb", "storage"),
        "the storehouse method fallback stays plain text: {recorded:?}"
    );
    assert!(
        card_run_at(&recorded, 32, 134, "#92ddea", "storage"),
        "the open-row storage label is the glossary term: {recorded:?}"
    );
}

#[test]
fn an_open_building_row_keeps_the_inverted_header_number_tints() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "announce_look()",
        "time(12, 2)",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "buildings");
    rig.opened();
    click_look(&mut rig, "rows");
    click_panel_point(&mut rig, 40.0, 123.0);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded.iter().any(|line| line.contains("#4fb3c8")),
        "the open row keeps its cyan band: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#15110a", "farm"),
        "open header words stay dark: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#663069", "80"),
        "open header digits use highlight_number: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#15110a", "/"),
        "open header punctuation stays dark: {recorded:?}"
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

fn ledger_content_width(rig: &Rig) -> f64 {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the ledger geometry");
    let content = line.split("content=").nth(1).expect("the content geometry");
    content.split_whitespace().next().unwrap().parse().unwrap()
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
        initial.contains("name@1:left:#92ddea,people@257:right:#92ddea,food@291:right:#92ddea"),
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
        narrow.contains("people@170:right:#92ddea,food@204:right:#92ddea"),
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
fn selecting_a_ledger_place_renders_its_location_tab_from_the_game_place_view() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "note('location-opened')",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    // The fourth row is the two-pop monastery fixture (Cluain Mhic Nóis), after the heading,
    // Rowan and the empty place. This exercises the actual main.lua row-selection callback.
    click_ledger_row(&mut rig, 4, 1800, 900);
    rig.opened();
    let visible_rows = rows(&rig);
    assert!(
        visible_rows.iter().any(|line| line.starts_with("people:")),
        "the selected game's place view rendered its location tab: {visible_rows:?}"
    );
}

fn shipped_place_fixture() -> (HostFunction, String) {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let scenario_dir = assets.join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&scenario_dir).expect("the shipped Ireland scenario");
    let (state, _) = crate::game::economy::starting_economy(&assets, &scenario_dir, &scenario)
        .expect("the shipped starting economy");
    let rules = crate::game::economy::load_rules(&assets)
        .expect("the economy rules load")
        .expect("the shipped economy rules exist");
    let ids: Vec<String> = scenario
        .locations
        .iter()
        .map(|location| location.id.clone())
        .collect();
    let land = crate::game::economy::land_counts(&scenario, &rules.terrain_order)
        .expect("the shipped catchments load");
    let land_by_place = scenario
        .locations
        .iter()
        .zip(land)
        .map(|(location, counts)| (location.id.clone(), counts))
        .collect();
    let view = Arc::new(crate::game::view::build_with_land(
        Some(&rules),
        &state,
        &ids,
        &land_by_place,
        ck_sim::Date {
            year: 768,
            month: 5,
            day: 1,
            hour: 0,
        },
    ));
    let place_tables: BTreeMap<String, ConfigValue> = view
        .places
        .iter()
        .map(|place| (place.id.clone(), crate::game_api::place_table(place)))
        .collect();
    let actual_place: HostFunction = Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(id)),
            Some(ConfigValue::Integer(_revision)),
        ] = arguments
        else {
            return Err("game.place fixture expected id and revision".to_owned());
        };
        place_tables
            .get(id)
            .cloned()
            .map(|place| vec![place])
            .ok_or_else(|| format!("unexpected game.place id {id}"))
    });
    let escape_lua = |value: &str| value.replace('\\', "\\\\").replace('\'', "\\'");
    let place_rows = scenario
        .locations
        .iter()
        .map(|location| {
            let kind = &scenario.location_kinds.kinds[location.kind_index].id;
            format!(
                "{{id='{}', name='{}', kind='{}', size={}}}",
                escape_lua(&location.id),
                escape_lua(&location.name),
                escape_lua(kind),
                location.size
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    (actual_place, place_rows)
}

#[test]
fn selecting_a_shipped_ledger_place_renders_its_real_game_place_table() {
    let (actual_place, place_rows) = shipped_place_fixture();
    let setup = format!("place_data = {{{place_rows}}}; time(1, 1) announce_bars()");
    let observer = r"
local list = require('lib.list')
local real_list_new = list.new
list.new = function(spec)
  if spec.window.title ~= 'ledger' then
    local first = spec.items[1]
    if type(first) == 'table' then
      local cells = {}
      for _, cell in ipairs(first.cells) do cells[#cells + 1] = cell.text end
      first = table.concat(cells)
    end
    window.record('location tab ' .. first)
  end
  return real_list_new(spec)
end
";
    let script = script_with_prelude(
        observer,
        &[
            &setup,
            "announce_ledger()",
            "note('last place call ' .. place_calls[#place_calls])",
        ],
    );
    let mut rig =
        Rig::new_fixture_body_with_functions(&script, vec![("actual_place", actual_place)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let (origin_x, origin_y) = ledger_origin(&rig);
    rig.feed(&[scroll_lines(origin_x + 20.0, origin_y + 45.0, -1)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.feed(&[
        PointerEvent::Down(points(origin_x + 20.0, origin_y + 45.0)),
        PointerEvent::Up(points(origin_x + 20.0, origin_y + 45.0)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    let visible_rows = rows(&rig);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "last place call clonmacnoise@1"),
        "the scrolled ledger click fetched Cluain Mhic Nóis from the shipped view: {:?}",
        notes(&rig)
    );
    assert!(
        visible_rows.iter().any(|line| line.starts_with("people:")),
        "a scrolled ledger row opens its shipped game.place view: {visible_rows:?}"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("location tab people:")),
        "the window witness can log the first typed location row: {:?}",
        notes(&rig)
    );
}

#[test]
fn real_ledger_hover_uses_the_drawn_row_cell_after_scroll_and_resize() {
    let mut rig = rig(&[
        "use_many_ledger_places(); time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_ledger_cell_tips()",
        "announce_ledger()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    rig.opened();
    assert_ledger_heading_and_place_tooltips(&rig);
    let (origin_x, origin_y) = ledger_origin(&rig);
    let width = ledger_content_width(&rig);
    let people_point = (origin_x + width - 84.0, origin_y + 39.0);
    let row_key = rig
        .key_at(people_point.0, people_point.1)
        .expect("the second ledger row is hit");
    assert!(row_key.ends_with("_row_2"), "{row_key}");
    rig.feed(&[PointerEvent::Move(points(people_point.0, people_point.1))]);
    for _ in 0..35 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("source ")
            && line.contains("peasants 120|workers 80, dependents 40 part=2")),
        "the real pointer and tooltip update reach this row's people source: {:?}",
        notes(&rig)
    );

    hover_food_cell_after_delay(&mut rig, (origin_x + width - 20.0, origin_y + 39.0));

    rig.feed(&[scroll_lines(people_point.0, people_point.1, -1)]);
    // Let the changed scroll position publish its new row hit regions before asking the same screen point again.
    rig.deliver_fixture_frame_at(1800, 900, 2);
    // Scrolling moves the rows beneath a stationary pointer. Publish the same pointer position again so the real
    // router asks the row that is now under it, rather than retaining the pre-scroll hover target.
    // A one-point nudge and return produces distinct cursor events while ending at the exact original screen point.
    rig.feed(&[PointerEvent::Move(points(
        people_point.0 + 1.0,
        people_point.1,
    ))]);
    rig.feed(&[PointerEvent::Move(points(people_point.0, people_point.1))]);
    for _ in 0..35 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("source ")
            && line.contains("peasants 180|monks 5|workers 108, dependents 72 part=2")),
        "the same pointer position follows the scrolled row to Cards: {:?}",
        notes(&rig)
    );

    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    let (resized_x, resized_y) = ledger_origin(&rig);
    let resized_width = ledger_content_width(&rig);
    let resized_point = (resized_x + resized_width - 84.0, resized_y + 39.0);
    rig.feed(&[PointerEvent::Move(points(resized_point.0, resized_point.1))]);
    for _ in 0..35 {
        rig.deliver_fixture_frame_at(1200, 700, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("source ")
            && line.contains("peasants 120|workers 80, dependents 40 part=2")),
        "the moved ledger routes its resized people cell: {:?}",
        notes(&rig)
    );
}

fn assert_ledger_heading_and_place_tooltips(rig: &Rig) {
    let captured = notes(rig);
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip heading_food title=food days|")),
        "the ledger food heading opens the food-days explanation: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip place_name title=Rowan|")),
        "the place-name cell opens that place's location content: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip place_food peasant Ulaid 215 days part=3")),
        "the place food cell opens the food breakdown with cell part 3: {captured:?}"
    );
}

fn hover_food_cell_after_delay(rig: &mut Rig, point: (f64, f64)) {
    let before = notes(rig);
    let people_tooltip_panels = before
        .iter()
        .filter(|line| line.starts_with("tooltip_panel_quad "))
        .count();
    assert!(
        people_tooltip_panels > 0,
        "the people tooltip was drawn: {before:?}"
    );
    rig.feed(&[PointerEvent::Move(points(point.0, point.1))]);
    for _ in 0..10 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    let waiting = notes(rig);
    assert_eq!(
        waiting
            .iter()
            .filter(|line| line.starts_with("tooltip_panel_quad "))
            .count(),
        people_tooltip_panels,
        "moving from people to food does not draw a new tooltip before the restarted delay"
    );
    for _ in 0..25 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    let opened = notes(rig);
    assert!(
        opened
            .iter()
            .any(|line| line.starts_with("source ")
                && line.contains("peasant Ulaid 215 days part=3")),
        "after the restarted delay, the real food-cell hover opens place_food: {opened:?}"
    );
    assert!(
        opened
            .iter()
            .filter(|line| line.starts_with("tooltip_panel_quad "))
            .count()
            > people_tooltip_panels,
        "the food tooltip is drawn after the restarted delay"
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

#[test]
fn strict_limit_markers_reach_place_rows_classes_and_wrapping() {
    let mut rig = rig(&[r#"local place = VIEWS[1].rowan
place.crowding_pm = 1120
place.basic_housing_occupants, place.basic_housing_capacity = 310, 300
place.makeshift_housing_occupants, place.makeshift_housing_capacity = 40, 60
place.footprint_m2, place.prepared_m2, place.max_buildable_m2 = 9100, 9000, 9500
place.storage_used_l, place.storage_capacity_l, place.overflow_lost = 60010, 60000, 52
place.last_month.lost = {{good = "oats", units = 52}}
place.pops[1].crowding_pm = 1120
place.buildings[1].store_used_l, place.buildings[1].store_capacity_l = 60010, 60000
colour_watch = true
time(1, 1); select_place("rowan", "Rowan")
local location = require("lib.location")
local wrapped = location.rows(location.lines("location", game.place("rowan", 1)), 204)
for _, row in ipairs(wrapped) do for _, cell in ipairs(row.cells or {}) do
  if cell.over_limit then note("wrapped_limit " .. cell.text) end
  if cell.text:find("310", 1, true) then note("wrapped_numerator " .. tostring(cell.over_limit) .. " " .. cell.text) end
  if cell.text:find("300", 1, true) then note("wrapped_capacity " .. tostring(cell.over_limit) .. " " .. cell.text) end
end end
local tip = require("lib.tooltip")
local function marked(name, content)
  for _, line in ipairs(content or {}) do if type(line) == "table" then
    for _, span in ipairs(line) do
      if type(span) == "table" and span.over_limit then note("class_limit " .. name .. " " .. span.text) end
    end
  end end
end
for _, name in ipairs({"crowding", "pop_crowding", "housing", "storage", "store", "building_land", "overflow"}) do
  marked(name, tip.content(name, {place = "rowan", building = 1, pop = 1}))
end"#]);
    rig.opened();
    let recorded = notes(&rig);
    for (value, ink) in [
        ("112%", "#f66155"),
        ("310", "#f66155"),
        ("300", "#5fb35a"),
        ("40", "#5fb35a"),
        ("9100", "#f66155"),
        ("9000", "#5fb35a"),
        ("60010", "#f66155"),
        ("60000", "#5fb35a"),
        ("52", "#f66155"),
    ] {
        assert!(
            recorded
                .iter()
                .any(|line| line.contains(&format!("{ink}={value}"))),
            "{value} should use {ink}: {recorded:?}"
        );
    }
    for expected in [
        "class_limit crowding 112%",
        "class_limit pop_crowding 112%",
        "class_limit housing 310",
        "class_limit storage 60010",
        "class_limit store 60010",
        "class_limit building_land 9100",
        "class_limit overflow 52",
    ] {
        assert!(
            recorded.iter().any(|line| line == expected),
            "missing {expected}: {recorded:?}"
        );
    }
    assert!(
        recorded
            .iter()
            .any(|line| line == "wrapped_numerator true 310"),
        "the wrapped numerator keeps the over-limit marker: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "wrapped_capacity false /300"),
        "the wrapped denominator does not inherit the numerator marker: {recorded:?}"
    );
}

#[test]
fn values_at_their_limits_and_zero_overflow_keep_number_colour() {
    let mut rig = rig(&[r#"local place = VIEWS[1].rowan
place.crowding_pm = 1000
place.basic_housing_occupants, place.basic_housing_capacity = 300, 300
place.makeshift_housing_occupants, place.makeshift_housing_capacity = 40, 60
place.footprint_m2, place.prepared_m2, place.max_buildable_m2 = 9000, 9000, 9500
place.storage_used_l, place.storage_capacity_l, place.overflow_lost = 60000, 60000, 0
place.last_month.lost = {}
colour_watch = true; time(1, 1); select_place("rowan", "Rowan")"#]);
    rig.opened();
    let recorded = notes(&rig);
    for value in ["100%", "300", "60000", "9000", "0"] {
        assert!(
            recorded
                .iter()
                .any(|line| line.contains(&format!("#5fb35a={value}"))),
            "at-limit or zero value {value} stays in number colour: {recorded:?}"
        );
    }
}

#[test]
fn normalized_tooltip_spans_colour_only_numbers_marked_past_the_limit() {
    let mut rig = rig(&[r#"colour_watch = true
local text = require("lib.text")
local real_set = text.set
text.set = function(line, value)
  real_set(line, value)
  if line.font_name ~= nil then note("tooltip_span " .. line.colour .. "=" .. value) end
end
local tip = require("lib.tooltip")
tip.attach("ck283_limit_span", function()
  return {{{text = "112%", over_limit = true}, " / ", "100%"}}
end)
tip.update({width = 1800, height = 900, dt = 1,
  pointer = {over = "ck283_limit_span", x = 500, y = 200, events = {}}})"#]);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_span #f66155=112%"),
        "the normalized over-limit span colours its numeric run: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_span #ece3cb= / "),
        "ordinary tooltip text keeps its normal ink: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_span #5fb35a=100%"),
        "unmarked tooltip numbers remain green: {recorded:?}"
    );
}
