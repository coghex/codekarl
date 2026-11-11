-- The location panel (milestone 9, slice 3; docs/economy-design.md §3.11): a window that fills the left third of the screen with the
-- economy of the selected place, in three tabs shown as title-bar buttons: location, pops and buildings. Each tab is a scroll list
-- of text lines.
--
-- **The data path.** The panel reads `game.place(id, revision)`, the immutable view of that place at a revision `game_time` carried,
-- and never anything live. It remembers the newest revision it has been told of: a new selection reads that one, and a `game_time`
-- with another revision refreshes the open panel from its own. A date alone refreshes nothing.
local ui = require("lib.ui")
local windows = require("lib.window")
local lists = require("lib.list")
local tabs = require("lib.tabs")
local cards = require("lib.cards")
local button = require("lib.button")
local frame = require("lib.frame")
local text = require("lib.text")
local theme = require("lib.theme")

local P = ui.P

local location = {}
location.__index = location

local TABS = {
  {key = "location", label = "location", short = "loc", tiny = "l"},
  {key = "pops", label = "pops", short = "pop", tiny = "p"},
  {key = "buildings", label = "buildings", short = "bld", tiny = "b"},
}

-- A price given in thousandths: whole units, then the fraction without trailing zeros ("100", "100.5").
local function milli(value)
  local whole, part = value // 1000, value % 1000
  if part == 0 then
    return tostring(whole)
  end
  return (string.format("%d.%03d", whole, part):gsub("0+$", ""))
end

-- The arrow of last month's births against deaths: up, down or level.
local function trend(births, deaths)
  if births > deaths then
    return "^"
  elseif births < deaths then
    return "v"
  end
  return "="
end

local function percent(per_mille)
  return (per_mille // 10) .. "%"
end

local function view()
  local self = {objects = {}}
  function self:add(object)
    self.objects[#self.objects + 1] = object
    return object
  end
  function self:destroy()
    for index = #self.objects, 1, -1 do
      local object = self.objects[index]
      if object.destroy ~= nil then object:destroy() end
      self.objects[index] = nil
    end
  end
  return self
end

local function put_text(result, group, x, y, content, colour, align)
  local line = text.line(0, 0, group, colour or theme.colour("text"))
  text.set(line, content)
  if align == "right" then x = x - line.width end
  text.move(line, x * P, y * P)
  return result:add({destroy = function() text.destroy(line) end})
end

local function put_quad(result, group, x, y, width, height, colour)
  local quad = render.create_quad{
    texture = ui.texture("ui/theme/white.png"), x = x * P, y = y * P, width = width * P, height = height * P,
    group = group, tint = colour,
  }
  return result:add({destroy = function() render.destroy_quad(quad) end})
end

local function type_colour(kind)
  local ok, colour = pcall(theme.colour, "pop_" .. tostring(kind))
  return ok and colour or theme.colour("text")
end

local function draw_icon(result, group, name, x, y, colour)
  local quad = render.create_quad{
    texture = ui.texture("ui/theme/icon_" .. name .. ".png"), x = x * P, y = y * P,
    width = 9 * P, height = 11 * P, group = group, tint = colour,
  }
  result:add({destroy = function() render.destroy_quad(quad) end})
end

local function draw_bar(result, group, x, y, width, numerator, denominator)
  put_quad(result, group, x, y, width, 3, theme.colour("track"))
  if denominator <= 0 then return end
  local fill = math.min(width, math.max(0, math.floor(width * numerator / denominator)))
  if fill > 0 then put_quad(result, group, x, y, fill, 3, theme.colour("text")) end
end

local function methods_text(building)
  if #building.methods == 0 then return "storage" end
  local methods = {}
  for _, method in ipairs(building.methods) do
    methods[#methods + 1] = method.id .. " " .. method.levels
  end
  return table.concat(methods, ", ")
end

local function building_icon(kind)
  if kind == "farm" or kind == "holdings" or kind == "storehouse" then return kind end
  return nil
end

local function pop_card(pop)
  local colour = type_colour(pop.kind)
  return {
    height = 61,
    draw = function(group, x, y, width)
      local result = view()
      local dx, dy = x // P, y // P
      local colours = theme.kind("place")
      result:add(frame.new{x = x, y = y, width = width, height = 61 * P, cut = 3, corners = "tl_br", fill = colours.fill, line = colours.outline, group = group})
      put_quad(result, group, dx + 1, dy + 4, 2, 57, colour)
      draw_icon(result, group, "pops", dx + 5, dy + 3, colour)
      put_text(result, group, dx + 17, dy + 3, pop.kind, theme.colour("text"))
      put_text(result, group, dx + width // P - 5, dy + 3, tostring(pop.workers + pop.dependents), theme.colour("text"), "right")
      put_text(result, group, dx + 17, dy + 14, pop.culture .. " · " .. pop.religion, theme.colour("dim"))
      put_text(result, group, dx + width // P - 5, dy + 14,
        string.format("%dw %dd %s", pop.workers, pop.dependents, trend(pop.births, pop.deaths)), theme.colour("dim"), "right")
      put_text(result, group, dx + 5, dy + 25, "food", theme.colour("dim"))
      draw_bar(result, group, dx + 29, dy + 30, 40, math.min(pop.food_days, 120), 120)
      put_text(result, group, dx + 73, dy + 25, pop.food_days .. "d", theme.colour("text"))
      put_text(result, group, dx + 105, dy + 25, "job", theme.colour("dim"))
      draw_bar(result, group, dx + 123, dy + 30, 40, pop.employed, pop.workers)
      put_text(result, group, dx + 5, dy + 37, "needs", theme.colour("dim"))
      for index, need in ipairs({pop.necessities, pop.niceties, pop.luxuries}) do
        draw_bar(result, group, dx + 35 + (index - 1) * 12, dy + 42, 10, math.max(0, math.min(need, 1000)), 1000)
      end
      put_text(result, group, dx + 73, dy + 37, "mood", theme.colour("dim"))
      local mood = (pop.satisfaction >= 0 and "+" or "") .. pop.satisfaction
      put_text(result, group, dx + 101, dy + 37, mood, pop.satisfaction > 0 and theme.colour("good") or theme.colour("text"))
      put_text(result, group, dx + 5, dy + 48, "housing", theme.colour("dim"))
      put_text(result, group, dx + 47, dy + 48, (pop.basic_housing or 0) .. " + " .. (pop.makeshift_housing or 0), theme.colour("text"))
      put_text(result, group, dx + 105, dy + 48, "crowd", theme.colour("dim"))
      put_text(result, group, dx + 135, dy + 48,
        pop.crowding_pm ~= false and pop.crowding_pm ~= nil and percent(pop.crowding_pm) or "—", theme.colour("text"))
      return result
    end,
  }
end

local function building_card(building)
  local icon = building_icon(building.kind)
  return {
    height = #building.herds > 0 and 50 or 39,
    draw = function(group, x, y, width)
      local result = view()
      local dx, dy = x // P, y // P
      local colours = theme.kind("place")
      local w = width // P
      local height = (#building.herds > 0 and 50 or 39)
      result:add(frame.new{x = x, y = y, width = width, height = height * P, cut = 3, corners = "tl_br", fill = colours.fill, line = colours.outline, group = group})
      if icon ~= nil then draw_icon(result, group, icon, dx + 5, dy + 3, theme.colour("text")) end
      put_text(result, group, dx + 17, dy + 3, building.kind, theme.colour("text"))
      put_text(result, group, dx + w - 5, dy + 3, "wage " .. milli(building.wage), theme.colour("dim"), "right")
      put_text(result, group, dx + 17, dy + 14, methods_text(building), theme.colour("dim"))
      put_text(result, group, dx + w - 5, dy + 14, "housing " .. (building.housing_occupants or 0), theme.colour("dim"), "right")
      put_text(result, group, dx + 5, dy + 25, "staff", theme.colour("dim"))
      draw_bar(result, group, dx + 33, dy + 30, 40, building.filled, building.filled + building.open)
      put_text(result, group, dx + 77, dy + 25, building.filled .. "/" .. (building.filled + building.open), theme.colour("text"))
      put_text(result, group, dx + 115, dy + 25, "store", theme.colour("dim"))
      draw_bar(result, group, dx + 145, dy + 30, 30, building.store_used_l, building.store_capacity_l)
      local hx = dx + 5
      for _, herd in ipairs(building.herds) do
        local label = herd.species .. " " .. herd.head .. "/" .. herd.maximum
        put_text(result, group, hx, dy + 37, label, herd.head < herd.maximum and theme.colour("dim") or theme.colour("text"))
        hx = hx + font.layout(label).width + 8
      end
      return result
    end,
  }
end

local function pop_row(pop, open)
  local lines = {
    pop.culture .. " · " .. pop.religion,
    "workers " .. pop.workers .. ", dependents " .. pop.dependents,
    "employed " .. pop.employed .. ", unemployed " .. pop.unemployed,
    "needs " .. percent(pop.necessities) .. " " .. percent(pop.niceties) .. " " .. percent(pop.luxuries),
    "births " .. pop.births .. ", deaths " .. pop.deaths,
    "housing: basic " .. (pop.basic_housing or 0) .. ", makeshift " .. (pop.makeshift_housing or 0),
    "crowding: " .. (pop.crowding_pm ~= false and pop.crowding_pm ~= nil and percent(pop.crowding_pm) or "—"),
  }
  return {
    height = open and (13 + 12 * #lines + 3) or 13,
    draw = function(group, x, y, width)
      local result = view()
      local dx, dy = x // P, y // P
      local ink = open and theme.kind("place").title_text or theme.colour("text")
      put_quad(result, group, dx, dy, width // P, 13, open and theme.kind("place").outline or theme.colour("band"))
      put_quad(result, group, dx, dy, 2, 13, type_colour(pop.kind))
      draw_icon(result, group, "pops", dx + 4, dy + 1, open and ink or type_colour(pop.kind))
      put_text(result, group, dx + 16, dy + 1, pop.kind .. " " .. pop.culture, ink)
      put_text(result, group, dx + width // P - 64, dy + 1, tostring(pop.workers + pop.dependents), ink, "right")
      put_text(result, group, dx + width // P - 32, dy + 1, pop.food_days .. "d", ink, "right")
      put_text(result, group, dx + width // P - 3, dy + 1, (pop.satisfaction >= 0 and "+" or "") .. pop.satisfaction, ink, "right")
      if open then
        for index, line in ipairs(lines) do put_text(result, group, dx + 16, dy + 13 + (index - 1) * 12, line, theme.colour("text")) end
      end
      return result
    end,
  }
end

local function building_row(building, open)
  local lines = {methods_text(building), "store " .. building.store_used_l .. " / " .. building.store_capacity_l .. " L"}
  for _, herd in ipairs(building.herds) do lines[#lines + 1] = herd.species .. " " .. herd.head .. " / " .. herd.maximum end
  lines[#lines + 1] = "positions " .. building.filled .. " filled, " .. building.open .. " open"
  lines[#lines + 1] = "housing occupants " .. (building.housing_occupants or 0)
  return {
    height = open and (13 + 12 * #lines + 3) or 13,
    draw = function(group, x, y, width)
      local result = view()
      local dx, dy = x // P, y // P
      local ink = open and theme.kind("place").title_text or theme.colour("text")
      put_quad(result, group, dx, dy, width // P, 13, open and theme.kind("place").outline or theme.colour("band"))
      local icon = building_icon(building.kind)
      if icon ~= nil then draw_icon(result, group, icon, dx + 3, dy + 1, ink) end
      put_text(result, group, dx + 16, dy + 1, building.kind, ink)
      put_text(result, group, dx + width // P - 44, dy + 1, building.filled .. "/" .. (building.filled + building.open), ink, "right")
      put_text(result, group, dx + width // P - 3, dy + 1, milli(building.wage), ink, "right")
      if open then
        for index, line in ipairs(lines) do put_text(result, group, dx + 16, dy + 13 + (index - 1) * 12, line, theme.colour("text")) end
      end
      return result
    end,
  }
end

local function location_lines(place)
  local lines = {
    "people: " .. (place.workers + place.dependents),
    "  workers: " .. place.workers,
    "  dependents: " .. place.dependents,
    "housing: basic " .. (place.basic_housing_occupants or 0) .. "/" .. (place.basic_housing_capacity or 0)
      .. ", makeshift " .. (place.makeshift_housing_occupants or 0) .. "/" .. (place.makeshift_housing_capacity or 0),
    "crowding: " .. percent(place.crowding_pm or 0),
    "food: " .. place.food_days .. " days",
    "building land: " .. place.footprint_m2 .. " / " .. math.min(place.prepared_m2, place.max_buildable_m2) .. " m²",
    "storage: " .. place.storage_used_l .. " / " .. place.storage_capacity_l .. " L",
    "lost to overflow last month: " .. place.overflow_lost,
    "prices (base):",
  }
  for _, price in ipairs(place.prices) do
    lines[#lines + 1] = "  " .. price.good .. " " .. milli(price.price_milli) .. " (" .. price.base .. ")"
  end
  return lines
end

local function pops_lines(place)
  if #place.pops == 0 then
    return {"no pops"}
  end
  local lines = {}
  for index, pop in ipairs(place.pops) do
    if index > 1 then
      lines[#lines + 1] = ""
    end
    lines[#lines + 1] = pop.kind .. " " .. pop.culture .. " " .. pop.religion
    lines[#lines + 1] = "  workers: " .. pop.workers
    lines[#lines + 1] = "  dependents: " .. pop.dependents
    lines[#lines + 1] = "  housing: basic " .. (pop.basic_housing or 0) .. ", makeshift " .. (pop.makeshift_housing or 0)
    lines[#lines + 1] = "  crowding: " .. (pop.crowding_pm ~= false and percent(pop.crowding_pm or 0) or "—")
    lines[#lines + 1] = "  employed: " .. pop.employed
    lines[#lines + 1] = "  unemployed: " .. pop.unemployed
    lines[#lines + 1] = "  food: " .. pop.food_days .. " days"
    lines[#lines + 1] = "  needs: " .. percent(pop.necessities) .. " " .. percent(pop.niceties) .. " " .. percent(pop.luxuries)
    lines[#lines + 1] = "  satisfaction: " .. pop.satisfaction
    lines[#lines + 1] = "  births " .. pop.births .. ", deaths " .. pop.deaths .. " " .. trend(pop.births, pop.deaths)
  end
  return lines
end

local function buildings_lines(place)
  if #place.buildings == 0 then
    return {"no buildings"}
  end
  local lines = {}
  for index, building in ipairs(place.buildings) do
    if index > 1 then
      lines[#lines + 1] = ""
    end
    lines[#lines + 1] = building.kind
    if #building.methods == 0 then
      lines[#lines + 1] = "  levels: " .. building.levels
    else
      local methods = {}
      for _, method in ipairs(building.methods) do
        methods[#methods + 1] = method.id .. " " .. method.levels
      end
      lines[#lines + 1] = "  levels: " .. table.concat(methods, ", ")
    end
    lines[#lines + 1] = "  positions: " .. building.filled .. " filled, " .. building.open .. " open"
    lines[#lines + 1] = "  housing occupants: " .. (building.housing_occupants or 0)
    for _, herd in ipairs(building.herds) do
      lines[#lines + 1] = "  " .. herd.species .. ": " .. herd.head .. " / " .. herd.maximum
    end
    lines[#lines + 1] = "  store: " .. building.store_used_l .. " / " .. building.store_capacity_l .. " L"
    lines[#lines + 1] = "  wage: " .. building.wage
  end
  return lines
end

local FORMATTERS = {
  location = location_lines,
  pops = pops_lines,
  buildings = buildings_lines,
}

-- The lines of tab `key` (`"location"`, `"pops"` or `"buildings"`) for `place`, a table `game.place` returned.
function location.lines(key, place)
  local format = FORMATTERS[key]
  if format == nil then
    error("location.lines: `" .. tostring(key) .. "` is not a tab", 2)
  end
  return format(place)
end

-- Breaks a line that is wider than `width` font pixels at its spaces (a word wider than the width is cut), continuing under a
-- deeper indent. Returns the rows.
local function wrap(line, width)
  if font.layout(line).width <= width then
    return {line}
  end
  local indent = line:match("^ *")
  local continued = indent .. "  "
  local rows = {}
  local row = nil
  for word in line:sub(#indent + 1):gmatch("%S+") do
    local lead = (#rows == 0) and indent or continued
    local candidate = row ~= nil and (row .. " " .. word) or (lead .. word)
    if font.layout(candidate).width <= width then
      row = candidate
    else
      if row ~= nil then
        rows[#rows + 1] = row
        row = nil
        lead = continued
      end
      -- A word that alone is too wide is cut into pieces, only between characters: a byte cut inside one (a fada) is not text.
      while font.layout(lead .. word).width > width and utf8.len(word) > 1 do
        local cut = utf8.len(word) - 1
        while cut > 1 and font.layout(lead .. word:sub(1, utf8.offset(word, cut + 1) - 1)).width > width do
          cut = cut - 1
        end
        local at = utf8.offset(word, cut + 1)
        rows[#rows + 1] = lead .. word:sub(1, at - 1)
        word = word:sub(at)
        lead = continued
      end
      row = lead .. word
    end
  end
  if row ~= nil then
    rows[#rows + 1] = row
  end
  return rows
end

-- The wrapped rows of `lines` for a list `width` points wide.
function location.rows(lines, width)
  local limit = width // P - 2
  local rows = {}
  for _, line in ipairs(lines) do
    for _, row in ipairs(wrap(line, limit)) do
      rows[#rows + 1] = row
    end
  end
  return rows
end

-- A panel over the windows of `spec.view` (a table with the window's current `width` and `height` in points, kept up to date by the
-- caller) that sits `spec.top()` points from the top of the screen (below the time bar, say). `spec.on_close` is called when the
-- window closes.
function location.new(spec)
  local self = setmetatable({}, location)
  self.view = spec.view
  self.on_close = spec.on_close
  self.top = spec.top or function() return 0 end
  self.window = nil
  self.list = nil
  self.cards = nil
  self.selector = nil
  self.tabs = nil
  self.id = nil
  self.revision = nil
  self.open_pops = nil
  self.open_buildings = nil
  self.key = TABS[1].key
  self.list_look = "cards"
  return self
end

local function open(self)
  return self.window ~= nil and not self.window.closed
end

-- The narrowest the panel gets, in points: what holds its three tabs (with their tiny labels), the close button and the frame.
function location.min_width()
  return windows.min_outer_width(tabs.reserved(TABS), false)
end

-- The outer size of the panel and where it sits: the left third of the screen, from `top` to the bottom edge. Where the third is
-- narrower than `min_width` (a very narrow window or a large scale) the panel is that wide, and may pass the window's edge.
local function geometry(self)
  local top = ui.snap(self.top())
  local width = math.max(ui.snap_size(self.view.width // 3), ui.snap(location.min_width()))
  local height = ui.snap_size(math.max(self.view.height - top, 8 * P))
  return top, width, height
end

-- The place's view at the revision this panel knows, or nil with the reason when it has none.
local function fetch(self)
  if self.revision == nil or self.revision == 0 then
    return nil, "economy: not published yet"
  end
  return game.place(self.id, self.revision)
end

local render

local function destroy_cards(self)
  if self.cards ~= nil and not self.cards.closed then self.cards:destroy() end
  if self.selector ~= nil then self.selector:destroy() end
  self.cards, self.selector = nil, nil
end

local function ensure_selector(self)
  if self.selector ~= nil then return end
  local self_owner = self
  local selector = {groups = {}, buttons = {}}
  local function choose(key)
    if self_owner.list_look == key then return end
    self_owner.list_look = key
    for name, control in pairs(selector.buttons) do control:set_selected(name == self_owner.list_look) end
    if self_owner.cards ~= nil and not self_owner.cards.closed then self_owner.cards:set_pos(0) end
    render(self_owner, false)
  end
  for _, key in ipairs({"cards", "rows"}) do
    selector.buttons[key] = button.new{
      x = 0, y = 0, icons = {key}, icon = key, kind = "place", style = "panel", group = self.window.content,
      owner = self.window, on_click = function() choose(key) end,
    }
    selector.buttons[key]:set_selected(key == self.list_look)
  end
  function selector:layout(_, _, _)
    local y = math.max(0, self_owner.window.content_height - 17 * P)
    self.buttons.cards:move(0, y)
    self.buttons.rows:move(16 * P, y)
  end
  function selector:destroy()
    for _, control in pairs(self.buttons) do control:destroy() end
    self.buttons = {}
  end
  self.selector = selector
  self.window:attach(selector)
end

local function text_view(group, x, y, content, colour, align)
  local result = view()
  put_text(result, group, x, y, content, colour, align)
  return result
end

local function row_identity(kind, item, index)
  return kind .. ":" .. index .. ":" .. tostring(item.kind) .. ":"
    .. tostring(item.culture or (item.methods[1] ~= nil and item.methods[1].id) or "")
end

local function row_items(self, key, place)
  local items = {}
  local function header(columns)
    items[#items + 1] = {
      height = 13,
      draw = function(group, _, _, width)
        local result = view()
        for _, cell in ipairs(columns) do
          local x = cell.right and width // P - cell.right or cell.x
          put_text(result, group, x, 1, cell.text, theme.colour("dim"), cell.right and "right" or nil)
        end
        return result
      end,
    }
  end
  if key == "pops" then
    header({{text = "pop", x = 16}, {text = "people", right = 64}, {text = "food", right = 32}, {text = "mood", right = 3}})
    for index, pop in ipairs(place.pops) do
      local id = row_identity("p", pop, index)
      local item = pop_row(pop, id == self.open_pops)
      item.on_click = function()
        if self.open_pops == id then
          self.open_pops = nil
        else
          self.open_pops = id
        end
        render(self, true)
      end
      items[#items + 1] = item
    end
  else
    header({{text = "building", x = 16}, {text = "staff", right = 44}, {text = "wage", right = 3}})
    for index, building in ipairs(place.buildings) do
      local id = row_identity("b", building, index)
      local item = building_row(building, id == self.open_buildings)
      item.on_click = function()
        if self.open_buildings == id then
          self.open_buildings = nil
        else
          self.open_buildings = id
        end
        render(self, true)
      end
      items[#items + 1] = item
    end
  end
  return items
end

local function card_items(key, place, reason)
  if place == nil then
    return {{height = 13, draw = function(group) return text_view(group, 1, 1, reason or "economy: not published yet", theme.colour("text")) end}}
  end
  local items, source = {}, key == "pops" and place.pops or place.buildings
  if #source == 0 then
    return {{height = 13, draw = function(group)
      return text_view(group, 1, 1, key == "pops" and "no pops" or "no buildings", theme.colour("dim"))
    end}}
  end
  for _, item in ipairs(source) do items[#items + 1] = key == "pops" and pop_card(item) or building_card(item) end
  return items
end

-- Makes the list show the current tab's rows, keeping its scroll when `keep_scroll`.
render = function(self, keep_scroll)
  if not open(self) then
    return
  end
  local place, reason = fetch(self)
  if self.key == "pops" or self.key == "buildings" then
    if self.list ~= nil and not self.list.closed then self.list:destroy(); self.list = nil end
    ensure_selector(self)
    local strip_top = math.max(0, self.window.content_height - 19 * P)
    local items = self.list_look == "cards" and card_items(self.key, place, reason)
      or row_items(self, self.key, place or {pops = {}, buildings = {}})
    local gap = self.list_look == "cards" and 3 or 1
    if self.cards ~= nil and not self.cards.closed then
      self.cards:set_items(items, gap)
      if not keep_scroll then self.cards:set_pos(0) end
    else
      self.cards = cards.new{
        window = self.window, x = 0, y = 0, width = self.window.content_width, height = strip_top,
        gap = gap, items = items,
      }
    end
    return
  end
  destroy_cards(self)
  local lines = place ~= nil and location.lines(self.key, place) or {reason}
  local rows = location.rows(lines, self.window.content_width)
  if self.list ~= nil and not self.list.closed then
    self.list:set_items(rows)
    if not keep_scroll then
      self.list:set_pos(0)
    end
  else
    -- A list shows at most `MAX_ROWS` rows; a taller window leaves the rest of its content empty.
    local count = math.min(lists.MAX_ROWS, math.max(1, self.window.content_height // lists.ROW))
    self.list = lists.new{
      window = self.window,
      x = 0,
      y = 0,
      width = self.window.content_width,
      rows = count,
      items = rows,
    }
  end
end

-- Fits the window to the left third and rebuilds the list for the room it has.
local function fit(self)
  local top, width, height = geometry(self)
  local active = self.cards ~= nil and not self.cards.closed and self.cards or self.list
  local scroll = (active ~= nil and not active.closed) and active:scroll_position() or 0
  destroy_cards(self)
  if self.list ~= nil and not self.list.closed then
    self.list:destroy()
  end
  self.list = nil
  self.window:set_outer_size(width, height)
  self.window:move_to(0, top)
  render(self, true)
  local current = self.cards ~= nil and self.cards or self.list
  if current ~= nil then
    current:set_pos(scroll)
  end
end

-- Opens the panel for `selection` (a `game_select` table or a places-list entry: `id`, `name`, ...) or, when it is open, shows that
-- place in it, on the tab that is showing.
function location:show(selection, tab)
  if tab ~= nil then
    if FORMATTERS[tab] == nil then error("location:show: `" .. tostring(tab) .. "` is not a tab", 2) end
    self.key = tab
  end
  if self.id ~= nil and self.id ~= selection.id then
    self.open_pops, self.open_buildings = nil, nil
  end
  self.id = selection.id
  if not open(self) then
    local top, width, height = geometry(self)
    local created
    created = windows.new{
      title = selection.name,
      kind = "place",
      outer_width = width,
      outer_height = height,
      clip_title = true,
      x = 0,
      y = top,
      on_close = function()
        if self.window == created then
          self.window = nil
          self.list = nil
          self.cards = nil
          self.selector = nil
          self.tabs = nil
        end
        if self.on_close ~= nil then
          self.on_close()
        end
      end,
    }
    self.window = created
    self.tabs = tabs.new{
      window = created,
      items = TABS,
      selected = self.key,
      on_select = function(key)
        self.key = key
        render(self, false)
      end,
    }
    render(self, false)
    return
  end
  self.window:set_title(selection.name)
  self.window:raise()
  render(self, false)
end

-- Tells the panel the newest revision `game_time` carried. A new revision refreshes an open panel, keeping its scroll; the same one
-- (a date alone, a speed, a pause) does nothing.
function location:set_revision(revision)
  if revision == self.revision then
    return
  end
  self.revision = revision
  if open(self) then
    render(self, true)
  end
end

-- Re-lays the open panel out for the window's current size.
function location:relayout()
  if open(self) then
    fit(self)
  end
end

-- The tab showing (`"location"`, `"pops"` or `"buildings"`).
function location:tab()
  return self.key
end

-- The immutable economy revision the panel reads, shared with callers that render other views of the same places.
function location:current_revision()
  return self.revision
end

location.TABS = TABS
return location
