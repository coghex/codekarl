//! The location panel (milestone 9, slice 3) in a real UI VM: the shipped `scripts/ui/main.lua` run over a stub `game` whose
//! `game.place` serves fixture views, with the panel's list rows recorded as the lists are created and updated.
//!
//! A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use ck_lua::ConfigValue;
use ck_window::PointerEvent;
use std::fmt::Write;

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
tooltip_source_kinds = {}
capture_tooltip_sources = false
local tooltip = require("lib.tooltip")
local real_tooltip_attach = tooltip.attach
local real_tooltip_label = tooltip.label
tooltip.label = function(key, words)
  tooltip_sources[key] = function() return {words} end
  tooltip_source_kinds[key] = "label"
  return real_tooltip_label(key, words)
end
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
  tooltip_source_kinds[key] = "lockable"
  return real_tooltip_attach(key, source)
end
local real_tooltip_detach = tooltip.detach
tooltip.detach = function(key)
  tooltip_sources[key] = nil
  tooltip_source_kinds[key] = nil
  return real_tooltip_detach(key)
end
function note_tooltip_kind(name, key)
  note("tooltip_kind " .. name .. " " .. tostring(tooltip_source_kinds[key] or "none"))
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
  note_tooltip_kind("date", "date_readout")
  note_tooltip("yield", mode_buttons.yield.key)
  note_tooltip_kind("mode_yield", mode_buttons.yield.key)
  if date_readout_rect ~= nil then
    note(string.format("date_hit %g %g %g %g", date_readout_rect.x, date_readout_rect.y,
      date_readout_rect.width, date_readout_rect.height))
  end
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
local real_texture_load = texture.load
texture.load = function(name)
  if name:match("^ui/goods/") then note("goodsload " .. name) end
  return real_texture_load(name)
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
  local bar = item.bar ~= nil and string.format(";bar=%g,%g,%s", item.bar.x, item.bar.width, tostring(item.bar.value)) or ""
  return table.concat(cells, ",") .. ";stripe=" .. (item.stripe or "-") .. ";icon=" .. icon .. bar
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
    note_tooltip_kind(name, line.row_keys[row])
  end
  local ui_pixel = require("lib.ui").P
  local people_cells = line.items[1].cells
  tip(1, people_cells[1].x * ui_pixel + font.layout(people_cells[1].text).width * ui_pixel / 2, "people_word")
  tip(1, people_cells[2].x * ui_pixel + 1, "people_number")
  local food_row = line.items[6]
  note("food_row_type " .. tostring(type(food_row)))
  tip(6, food_row.cells[2].x * ui_pixel + 1, "food_number")
  note_tooltip("title", window_objects.Rowan.title_key)
  note_tooltip_kind("title", window_objects.Rowan.title_key)
  for row_index, row in ipairs(line.items) do
    if type(row) == "table" and row.tooltip ~= nil then
      local y = line.origin_y + line.y - line.s + (row_index - 0.5) * 26
      for cell_index, cell in ipairs(row.cells) do
        local x = line.origin_x + line.x + cell.x * ui_pixel + font.layout(cell.text).width * ui_pixel / 2
        note_tooltip("tab_" .. row_index .. "_" .. cell_index, line.row_keys[row_index], x, y)
        note_tooltip_kind("tab_" .. row_index .. "_" .. cell_index, line.row_keys[row_index])
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
    note_tooltip_kind(name, line.row_keys[row])
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
    note_tooltip_kind(name, line.row_keys[1])
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

fn rig_with_prelude_at(prelude: &str, steps: &[&str], width: u32, height: u32, scale: u32) -> Rig {
    let mut rig = Rig::new_fixture_body(&script_with_prelude(prelude, steps));
    rig.deliver_fixture_frame_at(width, height, scale);
    rig
}

fn rig_at(steps: &[&str], width: u32, height: u32, scale: u32) -> Rig {
    let mut rig = Rig::new_fixture_body(&script(steps));
    rig.deliver_fixture_frame_at(width, height, scale);
    rig
}

/// Observers for the list bars of the location panel (CK-296): the bar the shipped script made (`panel_bar`, the newest) and how many
/// were made, the rows it last was given and returned, the content class the last tooltip asked for, and where the picker's rows are.
/// The ledger has bars of its own, which these leave alone.
pub(super) const PANEL_BARS: &str = r#"
do
local listbar_library = require("lib.listbar")
local real_listbar_new = listbar_library.new
panel_bars = 0
listbar_library.new = function(spec)
  local created = real_listbar_new(spec)
  if spec.window.title == "ledger" then return created end
  panel_bars = panel_bars + 1
  panel_bar = created
  local real_view = created.view
  function created:view(rows)
    local shown = real_view(self, rows)
    panel_shown, panel_given = shown, rows
    return shown
  end
  return created
end
local tooltip_library = require("lib.tooltip")
local real_tooltip_content = tooltip_library.content
tooltip_library.content = function(class, args)
  last_content = {class = class, args = args}
  return real_tooltip_content(class, args)
end
spy_hits, spy_offsets = {}, {}
local real_spy_hit = render.set_hit
render.set_hit = function(key, rects, group)
  spy_hits[key] = rects ~= nil and {rects = rects, group = group} or nil
  return real_spy_hit(key, rects, group)
end
local real_spy_offset = render.set_group_offset
render.set_group_offset = function(group, x, y)
  spy_offsets[group] = {x, y}
  return real_spy_offset(group, x, y)
end
end
function hit_centre(key)
  local kept = spy_hits[key]
  local offset = spy_offsets[kept.group] or {0, 0}
  local rect = kept.rects[1]
  return rect.x + offset[1] + rect.width / 2, rect.y + offset[2] + rect.height / 2
end
-- Notes the bar's buttons (centre, and rectangle in content points), its choices, count and the scroll of `scroller` ("list" or "cards").
function announce_panel_bar(scroller)
  local ox, oy = panel_bar.window:content_origin()
  for name, control in pairs({filter = panel_bar.filter_button, sort = panel_bar.sort_button}) do
    local width, height = control:size()
    note(string.format("pbar_%s %g %g", name, ox + control.x + width / 2, oy + control.y + height / 2))
    note(string.format("pbar_%s_rect %g %g %g %g", name, control.x, control.y, width, height))
  end
  note(string.format("pbar_content %g %g", panel_bar.window.content_width, panel_bar.window.content_height))
  local scroll = scroller == "cards" and card_object:scroll_position() or (scroller == "list" and list_object:scroll_position() or -1)
  note("pstate ticks=" .. panel_bar.ticks .. " sort=" .. tostring(panel_bar.sort_index) .. " bars=" .. panel_bars
    .. " shown=" .. #panel_shown .. " given=" .. #panel_given .. " scroll=" .. scroll)
  note("pcount " .. tostring(panel_bar.count.ck_test_text or ""))
end
function announce_ppicker()
  local picker = panel_bar.picker
  local labels = {}
  for index, row in ipairs(picker.rows) do
    labels[index] = row.entry.label
    if row.key ~= nil then
      local x, y = hit_centre(row.key)
      note(string.format("pick|%s|%g|%g", row.entry.label, x, y))
    end
  end
  note("ppicker " .. picker.kind .. " " .. table.concat(labels, ","))
end
-- The market's rows as the list shows them: the goods' names in order (the figures and the header are not goods).
function note_order(name)
  local names = {}
  for _, row in ipairs(panel_shown) do names[#names + 1] = row.sort.name end
  note(name .. " " .. table.concat(names, ","))
end
function note_kinds(name)
  local kinds = {}
  for _, row in ipairs(panel_shown) do kinds[#kinds + 1] = row.item.kind .. "@" .. row.index end
  note(name .. " " .. table.concat(kinds, ","))
end
function expect_that(ok, message) if not ok then error(message, 2) end end
function eq_that(actual, wanted, message)
  if actual ~= wanted then error(message .. ": got " .. tostring(actual) .. ", wanted " .. tostring(wanted), 2) end
end
-- Pops with every field a card and a row read.
function make_pop(kind, workers, dependents, food, mood)
  return {kind = kind, culture = "ulaid", religion = "christianity", workers = workers, dependents = dependents,
    employed = workers, unemployed = 0, food_days = food, necessities = 1000, niceties = 0, luxuries = 0, satisfaction = mood,
    births = 0, deaths = 0, basic_housing = 0, makeshift_housing = 0, crowding_pm = 0}
end
"#;

/// The place `cards` with five pops (peasant 40 people, monk 5, peasant 120, noble 10, monk 5) and its fourth building the one with the
/// most positions filled and a wage of 3, as revision 1; revisions 2 and 3 have it with no monks.
pub(super) const PANEL_FIXTURE: &str = r#"
VIEWS[1].cards.pops = {
  make_pop("peasant", 30, 10, 50, 5), make_pop("monk", 3, 2, 300, -2), make_pop("peasant", 100, 20, 20, 9),
  make_pop("noble", 5, 5, 90, 1), make_pop("monk", 4, 1, 200, 3),
}
VIEWS[1].cards.buildings[4].filled = 99
VIEWS[1].cards.buildings[4].wage = 3000
do
  local no_monks = {}
  for key, value in pairs(VIEWS[1].cards) do no_monks[key] = value end
  no_monks.pops = {make_pop("peasant", 30, 10, 50, 5), make_pop("peasant", 100, 20, 20, 9)}
  VIEWS[2].cards = no_monks
  VIEWS[3] = VIEWS[2]
end
"#;

/// A rig on the shared panel fixture at 1800 by 900 points.
pub(super) fn panel_rig(steps: &[&str]) -> Rig {
    rig_with_prelude_at(
        &format!("{PANEL_BARS}\n{PANEL_FIXTURE}"),
        steps,
        1800,
        900,
        2,
    )
}

/// The last note that starts with `prefix`.
pub(super) fn pnote(rig: &Rig, prefix: &str) -> String {
    notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no note {prefix}: {:?}", notes(rig)))
}

/// The note `{name} x y` as a point.
pub(super) fn ppoint(rig: &Rig, name: &str) -> (f64, f64) {
    let line = pnote(rig, &format!("{name} "));
    let mut parts = line[name.len() + 1..].split_whitespace();
    (
        parts.next().unwrap().parse().unwrap(),
        parts.next().unwrap().parse().unwrap(),
    )
}

/// The centre of the picker row labelled `label`, as the last `announce_ppicker` marked it.
pub(super) fn ppick(rig: &Rig, label: &str) -> (f64, f64) {
    let prefix = format!("pick|{label}|");
    let line = pnote(rig, &prefix);
    let mut parts = line[prefix.len()..].split('|');
    (
        parts.next().unwrap().parse().unwrap(),
        parts.next().unwrap().parse().unwrap(),
    )
}

/// A click at a point and a frame at the rig's 1800 by 900.
pub(super) fn pclick_at(rig: &mut Rig, (x, y): (f64, f64)) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

pub(super) fn pclick(rig: &mut Rig, name: &str) {
    let at = ppoint(rig, name);
    pclick_at(rig, at);
}

pub(super) fn ppick_click(rig: &mut Rig, label: &str) {
    let at = ppick(rig, label);
    pclick_at(rig, at);
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

const LOCATION_ROWS: &str = "people: 120|  workers: 80|  dependents: 40|housing: basic 80/100, makeshift 10/20|crowding: 50%|food: 215 days|building land: 4200 / 24200 m²|storage: 5400 / 86400 L|lost to overflow last month: 0|good@16:left:#92ddea,price@212:right:#92ddea,base@238:right:#92ddea,buy/sell@246:left:#9a927e;stripe=-;icon=-|oats@16:left:#92ddea,100@212:right:-,100@238:right:-;stripe=-;icon=goods/oats;bar=246,46,nil|ale@16:left:#92ddea,2.5@212:right:-,2@238:right:-;stripe=-;icon=goods/ale;bar=246,46,nil";

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

// ---------------------------------------------------------------------------------------------
// The map-mode buttons (milestone 9, slice 8)
// ---------------------------------------------------------------------------------------------

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

mod cards;
mod ledger;
mod market_table;
mod mode_bar;
mod panel_layout_refresh;
mod rows;
mod scale_limits;
mod selectors_tabs;
mod tooltips;
