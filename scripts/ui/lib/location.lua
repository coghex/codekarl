-- The location panel (milestone 9, slice 3; docs/economy-design.md §3.11): a window that fills the left third of the screen with the
-- economy of the selected place, in three tabs shown as title-bar buttons: location, pops and buildings. Each tab is a scroll list
-- of text lines.
--
-- **The data path.** The panel reads `game.place(id, revision)`, the immutable view of that place at a revision `game_time` carried,
-- and never anything live. It remembers the newest revision it has been told of: a new selection reads that one, and a `game_time`
-- with another revision refreshes the open panel from its own. Hourly weather comes with the time snapshot.
local ui = require("lib.ui")
local windows = require("lib.window")
local lists = require("lib.list")
local scrollbar = require("lib.scrollbar")
local tabs = require("lib.tabs")
local cards = require("lib.cards")
local listbar = require("lib.listbar")
local button = require("lib.button")
local frame = require("lib.frame")
local text = require("lib.text")
local theme = require("lib.theme")
local list_look = require("lib.list_look")
local place_names = require("lib.place_names")
local tooltip = require("lib.tooltip")
local pointer = require("lib.pointer")

local P = ui.P

local location = {}
location.__index = location
local price_revision

local TABS = {
  {key = "location", label = "location", short = "loc", tiny = "l", label_tooltip = false},
  {key = "pops", label = "pops", short = "pop", tiny = "p"},
  {key = "buildings", label = "buildings", short = "bld", tiny = "b"},
}

-- The icon of the title bar's go-to button: the ledger's location icon (CK-381).
local GO_TO_ICON = "location"

local function location_tab_labels(name)
  local count = utf8.len(name) or #name
  local short = name
  if count > 12 then short = name:sub(1, utf8.offset(name, 13) - 1) .. "…" end
  local tiny = name
  if count > 1 then tiny = name:sub(1, utf8.offset(name, 2) - 1) end
  return name, short, tiny
end

local function update_location_tab(self, selection)
  local full, short, tiny = location_tab_labels(selection.name)
  self.tabs:set_labels("location", full, short, tiny)
  tooltip.attach(self.tabs.buttons[1].key, {class = "location", args = {place = selection.id}})
end

-- This list is deliberately fixed: an unlisted future good keeps its readable name without an asynchronous file lookup.
local GOODS_WITH_ICONS = {
  "oats", "barley", "peas_beans", "vegetables", "apples", "dairy", "beef", "pork", "mutton", "fish", "seabirds",
  "ale", "mead", "wine", "honey", "herbs", "wool", "beeswax", "hides", "linen", "hemp", "hardwood", "peat", "cattle", "pigs", "sheep", "horses",
}
local HAS_GOODS_ICON = {}
for _, good in ipairs(GOODS_WITH_ICONS) do HAS_GOODS_ICON[good] = true end

local function good_icon(good)
  return HAS_GOODS_ICON[good] and ("goods/" .. good) or "goods/fallback/unknown"
end

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

local function put_text(result, group, x, y, content, colour, align, number_colour)
  local base = colour or theme.colour("text")
  local line = text.line(0, 0, group, base)
  text.set_segments(line, text.colour_numbers(content, base, number_colour or theme.colour("number")))
  if align == "right" then x = x - line.width end
  text.move(line, x * P, y * P)
  return result:add({destroy = function() text.destroy(line) end})
end

local function put_term_text(result, group, x, y, segments, align, number_colour)
  local line = text.line(0, 0, group, theme.colour("text"))
  text.set_segments(line, text.colour_segments(segments, number_colour or theme.colour("number"), theme.colour("term")))
  if align == "right" then x = x - line.width end
  text.move(line, x * P, y * P)
  return result:add({destroy = function() text.destroy(line) end})
end

local function term(text_value)
  return {text = text_value, term = true}
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
    texture = ui.texture((name == "demesne" and "ui/location/icon_" or "ui/theme/icon_") .. name .. ".png"), x = x * P, y = y * P,
    width = 9 * P, height = 11 * P, group = group, tint = colour,
  }
  result:add({destroy = function() render.destroy_quad(quad) end})
end

local function draw_goods_icon(result, group, good, x, y)
  local quad = render.create_quad{
    texture = ui.texture("ui/" .. good_icon(good) .. ".png"), x = x * P, y = y * P,
    width = 9 * P, height = 9 * P, group = group,
  }
  result:add({destroy = function() render.destroy_quad(quad) end})
end

local GOOD_NAMES = {peas_beans = "peas and beans"}

local function good_name(id)
  return GOOD_NAMES[id] or id:gsub("_", " ")
end

local function within(x, y, left, top, right, bottom)
  return x >= left and x < right and y >= top and y < bottom
end

local function class_tip(class, place_id, key, index, part, extra)
  local args = {place = place_id}
  args[key] = index
  if extra ~= nil then args.species = extra end
  return tooltip.content(class, args), part
end

local function explanation_tip(key, part)
  return tooltip.explanation(key), part
end

local function method_label(method)
  local label = method.id .. " " .. method.levels
  if (method.paused or 0) > 0 then label = label .. " (" .. method.paused .. " paused)" end
  return label
end

local function method_tip(building, x, y, origin_x, origin_y)
  local left, top = origin_x or 17, origin_y or 14
  for _, method in ipairs(building.methods) do
    local label = method_label(method)
    local right = left + font.layout(label).width
    if within(x, y, left, top, right, top + font.layout(label).height) then
      return explanation_tip(method.id, method.id)
    end
    left = right + font.layout(", ").width
  end
  return nil
end

tooltip.define("price", function(args)
  if price_revision == nil or game.place == nil then return nil end
  local place = game.place(args.place, price_revision)
  for _, price in ipairs(place.prices) do
    if price.good == args.good then
      return {
        good_name(price.good) .. ": price " .. milli(price.price_milli),
        "base price " .. price.base,
        "bought (buy orders) " .. (price.bought == nil and "-" or tostring(price.bought)),
        "offered (sell orders) " .. (price.offered == nil and "-" or tostring(price.offered)),
      }
    end
  end
  return nil
end)

local function draw_bar(result, group, x, y, width, numerator, denominator)
  put_quad(result, group, x, y, width, 3, theme.colour("track"))
  if denominator <= 0 then return end
  local fill = require("lib.draw").bar_fill(numerator, denominator, width)
  if fill > 0 then put_quad(result, group, x, y, fill, 3, theme.colour("text")) end
end

local function methods_text(building)
  if #building.methods == 0 then return "storage" end
  local methods = {}
  for _, method in ipairs(building.methods) do
    methods[#methods + 1] = method_label(method)
  end
  return table.concat(methods, ", ")
end

local function building_icon(kind)
  if kind == "farm" or kind == "holdings" or kind == "storehouse" or kind == "demesne" then return kind end
  return nil
end

local function method_texture(id)
  return "ui/location/icon_method_" .. id .. ".png"
end

local function wage_text(building)
  if building.has_methods then return "offers" end
  return "wage " .. milli(building.wage)
end

local function card_layout(building, width)
  local housing = "housing " .. (building.housing_occupants or 0)
  local housing_left = width - 5 - font.layout(housing).width
  local wage_left = width - 5 - font.layout(wage_text(building)).width
  local kind = (building.abandoned and "abandoned " or (building.paused and "paused " or "")) .. building.kind
  while font.layout(kind).width > wage_left - 17 - 4 and utf8.len(kind) > 1 do
    kind = kind:sub(1, utf8.offset(kind, -1) - 1)
  end
  local mx = 17 + font.layout(kind).width + 4
  local method = #building.methods > 0 and {x = mx, y = 3} or nil
  if method ~= nil and mx + 9 + 8 > wage_left then method = {x = 5, y = 14} end
  local start = method ~= nil and method.y == 14 and 18 or 5
  local strip_width = 0
  for index, herd in ipairs(building.herds) do
    if index > 1 then strip_width = strip_width + 10 end
    strip_width = strip_width + 11 + font.layout(herd.head .. "/" .. herd.maximum).width
  end
  local bottom = #building.herds > 0 and start + strip_width + 8 > housing_left
  local herds, hx, hy = {}, bottom and 5 or start, bottom and 37 or 14
  for _, herd in ipairs(building.herds) do
    local w = 11 + font.layout(herd.head .. "/" .. herd.maximum).width
    if bottom and hx > 5 and hx + w > width - 5 then hx, hy = 5, hy + 11 end
    herds[#herds + 1] = {x = hx, y = hy, herd = herd}
    hx = hx + w + 10
  end
  return {method = method, herds = herds, kind = kind, housing_left = housing_left, height = bottom and (50 + hy - 37) or 39}
end

local function card_method_tip(building)
  local lines = {title = "methods"}
  for _, method in ipairs(building.methods) do
    lines[#lines + 1] = {{icon = method_texture(method.id)}, " ",
      {text = method.id:gsub("_", " "), term = method.id}, " level " .. method.levels .. ((method.paused or 0) > 0 and (" (" .. method.paused .. " paused)") or "")}
  end
  return lines, "methods"
end

-- Keep the card's third-line drawing and tooltip hit areas on one width-aware layout.
local function building_line_layout(building, width)
  local positions_width = font.layout("positions").width
  local storage_width = font.layout("storage").width
  local count = building.filled .. "/" .. (building.filled + building.open)
  local count_width = font.layout(count).width
  local fixed = 5 + positions_width + 4 + 4 + count_width + 12 + storage_width + 4 + 6
  local room = width - fixed
  local layout = {}
  if room >= 34 then
    layout.step = 1
    layout.positions = {x = 5, width = positions_width}
    layout.positions_bar = {x = 5 + positions_width + 4, width = 24}
    layout.count = {x = layout.positions_bar.x + 24 + 4, width = count_width, text = count}
    layout.storage = {x = layout.count.x + count_width + 12, width = storage_width}
    layout.storage_bar = {x = layout.storage.x + storage_width + 4, width = room - 24}
  elseif room >= 12 then
    layout.step = 2
    layout.positions = {x = 5, width = positions_width}
    local positions_bar_width = math.floor(room / 2)
    layout.positions_bar = {x = 5 + positions_width + 4, width = positions_bar_width}
    layout.count = {x = layout.positions_bar.x + positions_bar_width + 4, width = count_width, text = count}
    layout.storage = {x = layout.count.x + count_width + 12, width = storage_width}
    layout.storage_bar = {x = layout.storage.x + storage_width + 4, width = room - positions_bar_width}
  elseif width >= 5 + positions_width + 4 + count_width + 12 + storage_width + 6 then
    layout.step = 3
    layout.positions = {x = 5, width = positions_width}
    layout.count = {x = 5 + positions_width + 4, width = count_width, text = count}
    layout.storage = {x = layout.count.x + count_width + 12, width = storage_width}
  elseif width >= 5 + positions_width + 4 + count_width + 6 then
    layout.step = 4
    layout.positions = {x = 5, width = positions_width}
    layout.count = {x = 5 + positions_width + 4, width = count_width, text = count}
  elseif width >= 5 + positions_width + 6 then
    layout.step = 5
    layout.positions = {x = 5, width = positions_width}
  else
    layout.step = 6
  end
  return layout
end

local function pop_card(pop, place_id, pop_index)
  local colour = type_colour(pop.kind)
  local card_width
  return {
    height = 61,
    tooltip = function(x, y)
      if card_width == nil then return nil end
      local size = tostring(pop.workers + pop.dependents)
      local trend_text = string.format("%dw %dd %s", pop.workers, pop.dependents, trend(pop.births, pop.deaths))
      local food_text, job_text = tostring(pop.food_days) .. "d", "job"
      local mood = (pop.satisfaction >= 0 and "+" or "") .. pop.satisfaction
      local housing = (pop.basic_housing or 0) .. " + " .. (pop.makeshift_housing or 0)
      local crowd = pop.crowding_pm ~= false and pop.crowding_pm ~= nil and percent(pop.crowding_pm) or "—"
      if within(x, y, 5, 3, 14, 14) or within(x, y, 17, 3, 17 + font.layout(pop.kind).width, 3 + font.layout(pop.kind).height) then
        return explanation_tip(pop.kind, "kind")
      end
      local culture_left = 17
      local culture_right = culture_left + font.layout(pop.culture).width
      if within(x, y, culture_left, 14, culture_right, 14 + font.layout(pop.culture).height) then
        return explanation_tip(pop.culture, "culture")
      end
      local religion_left = culture_right + font.layout(" · ").width
      if within(x, y, religion_left, 14, religion_left + font.layout(pop.religion).width, 14 + font.layout(pop.religion).height) then
        return explanation_tip(pop.religion, "religion")
      end
      local size_right = card_width - 5
      local size_left = size_right - font.layout(size).width
      if within(x, y, size_left, 3, size_right, 3 + font.layout(size).height) then
        return class_tip("pop_size", place_id, "pop", pop_index, "size")
      end
      local trend_right = card_width - 5
      local trend_left = trend_right - font.layout(trend_text).width
      if within(x, y, trend_left, 14, trend_right, 14 + font.layout(trend_text).height) then
        return class_tip("pop_size", place_id, "pop", pop_index, "trend")
      end
      if within(x, y, 5, 25, 5 + font.layout("food").width, 25 + font.layout("food").height) then
        return explanation_tip("food_days", "food")
      end
      if within(x, y, 29, 30, 69, 33) then return class_tip("pop_food", place_id, "pop", pop_index, "food_bar") end
      if within(x, y, 73, 25, 73 + font.layout(food_text).width, 25 + font.layout(food_text).height) then
        return class_tip("pop_food", place_id, "pop", pop_index, "food_days")
      end
      if within(x, y, 105, 25, 105 + font.layout(job_text).width, 25 + font.layout(job_text).height) then
        return explanation_tip("jobs", "job")
      end
      if within(x, y, 123, 30, 163, 33) then return class_tip("jobs", place_id, "pop", pop_index, "job_bar") end
      if within(x, y, 5, 37, 5 + font.layout("needs").width, 37 + font.layout("needs").height) then
        return explanation_tip("needs", "needs")
      end
      if within(x, y, 35, 42, 69, 45) then return explanation_tip("needs", "needs_bar") end
      if within(x, y, 73, 37, 73 + font.layout("mood").width, 37 + font.layout("mood").height) then
        return explanation_tip("satisfaction", "mood")
      end
      if within(x, y, 101, 37, 101 + font.layout(mood).width, 37 + font.layout(mood).height) then
        return explanation_tip("satisfaction", "mood_value")
      end
      if within(x, y, 5, 48, 5 + font.layout("housing").width, 48 + font.layout("housing").height) then
        return explanation_tip("housing", "housing")
      end
      if within(x, y, 47, 48, 47 + font.layout(housing).width, 48 + font.layout(housing).height) then
        return class_tip("pop_housing", place_id, "pop", pop_index, "housing_value")
      end
      if within(x, y, 105, 48, 105 + font.layout("crowd").width, 48 + font.layout("crowd").height) then
        return explanation_tip("crowding", "crowd")
      end
      if within(x, y, 135, 48, 135 + font.layout(crowd).width, 48 + font.layout(crowd).height) then
        return class_tip("pop_crowding", place_id, "pop", pop_index, "crowd_value")
      end
      return nil
    end,
    draw = function(group, x, y, width)
      card_width = width // P
      local result = view()
      local dx, dy = x // P, y // P
      local colours = theme.kind("place")
      result:add(frame.new{x = x, y = y, width = width, height = 61 * P, cut = 3, corners = "tl_br", fill = colours.fill, line = colours.outline, group = group})
      put_quad(result, group, dx + 1, dy + 4, 2, 57, colour)
      draw_icon(result, group, "pops", dx + 5, dy + 3, colour)
      put_term_text(result, group, dx + 17, dy + 3, {term(pop.kind)})
      put_text(result, group, dx + width // P - 5, dy + 3, tostring(pop.workers + pop.dependents), theme.colour("text"), "right")
      put_term_text(result, group, dx + 17, dy + 14,
        {term(pop.culture), {text = " · ", colour = theme.colour("dim")}, term(pop.religion)})
      put_text(result, group, dx + width // P - 5, dy + 14,
        string.format("%dw %dd %s", pop.workers, pop.dependents, trend(pop.births, pop.deaths)), theme.colour("dim"), "right")
      put_term_text(result, group, dx + 5, dy + 25, {term("food")})
      draw_bar(result, group, dx + 29, dy + 30, 40, math.min(pop.food_days, 120), 120)
      put_text(result, group, dx + 73, dy + 25, pop.food_days .. "d", theme.colour("text"))
      put_term_text(result, group, dx + 105, dy + 25, {term("job")})
      draw_bar(result, group, dx + 123, dy + 30, 40, pop.employed, pop.workers)
      put_term_text(result, group, dx + 5, dy + 37, {term("needs")})
      for index, need in ipairs({pop.necessities, pop.niceties, pop.luxuries}) do
        draw_bar(result, group, dx + 35 + (index - 1) * 12, dy + 42, 10, math.max(0, math.min(need, 1000)), 1000)
      end
      put_term_text(result, group, dx + 73, dy + 37, {term("mood")})
      local mood = (pop.satisfaction >= 0 and "+" or "") .. pop.satisfaction
      put_text(result, group, dx + 101, dy + 37, mood, theme.colour("text"))
      put_term_text(result, group, dx + 5, dy + 48, {term("housing")})
      put_text(result, group, dx + 47, dy + 48, (pop.basic_housing or 0) .. " + " .. (pop.makeshift_housing or 0), theme.colour("text"))
      put_term_text(result, group, dx + 105, dy + 48, {term("crowd")})
      local crowding_over = pop.crowding_pm ~= false and pop.crowding_pm ~= nil and pop.crowding_pm > 1000
      put_text(result, group, dx + 135, dy + 48,
        pop.crowding_pm ~= false and pop.crowding_pm ~= nil and percent(pop.crowding_pm) or "—",
        theme.colour("text"), nil, crowding_over and theme.colour("over_limit") or nil)
      return result
    end,
  }
end

local function building_card(building, place_id, building_index)
  local icon = building_icon(building.kind)
  local card_width
  return {
    height = 39,
    measure = function(width) return card_layout(building, width // P).height end,
    tooltip = function(x, y)
      if card_width == nil then return nil end
      local layout = card_layout(building, card_width)
      if within(x, y, 5, 3, 14, 14) or within(x, y, 17, 3, 17 + font.layout(layout.kind).width, 3 + font.layout(layout.kind).height) then
        return explanation_tip(building.kind, "kind")
      end
      local wage_label = wage_text(building)
      local wage_left = card_width - 5 - font.layout(wage_label).width
      if building.has_methods and within(x, y, wage_left, 3, card_width - 5, 3 + font.layout(wage_label).height) then
        return class_tip("wage", place_id, "building", building_index, "wage")
      end
      if not building.has_methods and within(x, y, wage_left, 3, wage_left + font.layout("wage ").width, 3 + font.layout(wage_label).height) then
        return explanation_tip("wage", "wage_word")
      end
      if not building.has_methods and within(x, y, wage_left + font.layout("wage ").width, 3, card_width - 5, 3 + font.layout(wage_label).height) then
        return class_tip("wage", place_id, "building", building_index, "wage")
      end
      local method = layout.method
      if method ~= nil and within(x, y, method.x, method.y, method.x + 9, method.y + 11) then
        return card_method_tip(building)
      end
      local housing_text = "housing " .. (building.housing_occupants or 0)
      local housing_left = card_width - 5 - font.layout(housing_text).width
      if within(x, y, housing_left, 14, card_width - 5, 14 + font.layout(housing_text).height) then
        local label_width = font.layout("housing ").width
        if x < housing_left + label_width then return explanation_tip("housing", "housing") end
        return explanation_tip("housing", "housing_value")
      end
      local line = building_line_layout(building, card_width)
      if line.positions ~= nil and within(x, y, line.positions.x, 25,
          line.positions.x + line.positions.width, 25 + font.layout("positions").height) then
        return explanation_tip("position", "positions")
      end
      if line.positions_bar ~= nil and within(x, y, line.positions_bar.x, 30,
          line.positions_bar.x + line.positions_bar.width, 33) then
        return class_tip("staff", place_id, "building", building_index, "staff_bar")
      end
      if line.count ~= nil and within(x, y, line.count.x, 25, line.count.x + line.count.width,
          25 + font.layout(line.count.text).height) then
        return class_tip("staff", place_id, "building", building_index, "staff_count")
      end
      if line.storage ~= nil and within(x, y, line.storage.x, 25,
          line.storage.x + line.storage.width, 25 + font.layout("storage").height) then
        return explanation_tip("storage", "storage")
      end
      if line.storage_bar ~= nil and within(x, y, line.storage_bar.x, 30,
          line.storage_bar.x + line.storage_bar.width, 33) then
        return class_tip("store", place_id, "building", building_index, "store_bar")
      end
      for _, entry in ipairs(layout.herds) do
        local herd, hx, hy = entry.herd, entry.x, entry.y
        local count = herd.head .. "/" .. herd.maximum
        if within(x, y, hx, hy + 1, hx + 9, hy + 10) then
          return explanation_tip(herd.species, herd.species)
        end
        if within(x, y, hx + 11, hy, hx + 11 + font.layout(count).width, hy + font.layout(count).height) then
          return class_tip("herd", place_id, "building", building_index, herd.species, herd.species)
        end
      end
      return nil
    end,
    draw = function(group, x, y, width)
      card_width = width // P
      local result = view()
      local dx, dy = x // P, y // P
      local colours = theme.kind("place")
      local w = width // P
      local layout = card_layout(building, w)
      local height = layout.height
      result:add(frame.new{x = x, y = y, width = width, height = height * P, cut = 3, corners = "tl_br", fill = colours.fill, line = colours.outline, group = group})
      if icon ~= nil then draw_icon(result, group, icon, dx + 5, dy + 3, theme.colour("text")) end
      put_term_text(result, group, dx + 17, dy + 3, {term(layout.kind)})
      put_term_text(result, group, dx + w - 5, dy + 3,
        {{text = wage_text(building), colour = theme.colour("dim")}}, "right")
      if layout.method ~= nil then
        local method_id = #building.methods > 1 and "split" or building.methods[1].id
        local quad = render.create_quad{texture = ui.texture(method_texture(method_id)),
          x = (dx + layout.method.x) * P, y = (dy + layout.method.y) * P,
          width = 9 * P, height = 11 * P, group = group}
        result:add({destroy = function() render.destroy_quad(quad) end})
      end
      put_term_text(result, group, dx + w - 5, dy + 14,
        {term("housing "), {text = tostring(building.housing_occupants or 0), colour = theme.colour("dim")}}, "right")
      local line = building_line_layout(building, w)
      if line.positions ~= nil then
        put_term_text(result, group, dx + line.positions.x, dy + 25, {term("positions")})
      end
      if line.positions_bar ~= nil then
        draw_bar(result, group, dx + line.positions_bar.x, dy + 30, line.positions_bar.width,
          building.filled, building.filled + building.open)
      end
      if line.count ~= nil then
        put_text(result, group, dx + line.count.x, dy + 25, line.count.text, theme.colour("text"))
      end
      if line.storage ~= nil then
        put_term_text(result, group, dx + line.storage.x, dy + 25, {term("storage")})
      end
      if line.storage_bar ~= nil then
        draw_bar(result, group, dx + line.storage_bar.x, dy + 30, line.storage_bar.width,
          building.store_used_l, building.store_capacity_l)
      end
      for _, entry in ipairs(layout.herds) do
        local herd, hx, hy = entry.herd, dx + entry.x, dy + entry.y
        local count = herd.head .. "/" .. herd.maximum
        draw_goods_icon(result, group, herd.species, hx, hy + 1)
        put_text(result, group, hx + 11, hy, count, herd.head < herd.maximum and theme.colour("dim") or theme.colour("text"))
      end
      return result
    end,
  }
end

local function pop_row(pop, open, place_id, pop_index)
  local lines = {
    pop.culture .. " · " .. pop.religion,
    "workers " .. pop.workers .. ", dependents " .. pop.dependents,
    "employed " .. pop.employed .. ", unemployed " .. pop.unemployed,
    "needs " .. percent(pop.necessities) .. " " .. percent(pop.niceties) .. " " .. percent(pop.luxuries),
    "births " .. pop.births .. ", deaths " .. pop.deaths,
    "housing: basic " .. (pop.basic_housing or 0) .. ", makeshift " .. (pop.makeshift_housing or 0),
    "crowding: " .. (pop.crowding_pm ~= false and pop.crowding_pm ~= nil and percent(pop.crowding_pm) or "—"),
  }
  local row_width
  local function summary_tip(x, y)
    if y < 1 or y >= 13 then return nil end
    if within(x, y, 4, 1, 13, 12) then return explanation_tip(pop.kind, "kind") end
    local kind_left = 16
    local culture_left = kind_left + font.layout(pop.kind .. " ").width
    if within(x, y, kind_left, 1, culture_left - font.layout(" ").width, 1 + font.layout(pop.kind).height) then
      return explanation_tip(pop.kind, "kind")
    end
    if within(x, y, culture_left, 1, culture_left + font.layout(pop.culture).width, 1 + font.layout(pop.culture).height) then
      return explanation_tip(pop.culture, "culture")
    end
    local size = tostring(pop.workers + pop.dependents)
    local size_right, size_left = row_width - 64, row_width - 64 - font.layout(size).width
    if within(x, y, size_left, 1, size_right, 1 + font.layout(size).height) then
      return class_tip("pop_size", place_id, "pop", pop_index, "size")
    end
    local food = tostring(pop.food_days) .. "d"
    local food_right, food_left = row_width - 32, row_width - 32 - font.layout(food).width
    if within(x, y, food_left, 1, food_right, 1 + font.layout(food).height) then
      return class_tip("pop_food", place_id, "pop", pop_index, "food_days")
    end
    local mood = (pop.satisfaction >= 0 and "+" or "") .. pop.satisfaction
    local mood_right, mood_left = row_width - 3, row_width - 3 - font.layout(mood).width
    if within(x, y, mood_left, 1, mood_right, 1 + font.layout(mood).height) then
      return explanation_tip("satisfaction", "mood")
    end
    return nil
  end
  return {
    height = open and (13 + 12 * #lines + 3) or 13,
    tooltip = function(x, y)
      if row_width == nil then return nil end
      if open then
        if y < 13 then return summary_tip(x, y) end
        local line_index = math.floor((y - 13) / 12) + 1
        if line_index < 1 or line_index > #lines then return nil end
        if line_index == 1 then
          local culture_left = 16
          local culture_right = culture_left + font.layout(pop.culture).width
          if within(x, y, culture_left, 13, culture_right, 13 + font.layout(pop.culture).height) then
            return explanation_tip(pop.culture, "culture")
          end
          local religion_left = culture_right + font.layout(" · ").width
          if within(x, y, religion_left, 13, religion_left + font.layout(pop.religion).width, 13 + font.layout(pop.religion).height) then
            return explanation_tip(pop.religion, "religion")
          end
          return nil
        elseif line_index == 2 or line_index == 5 then
          return class_tip("pop_size", place_id, "pop", pop_index, line_index == 2 and "size" or "trend")
        elseif line_index == 3 then
          return class_tip("jobs", place_id, "pop", pop_index, "jobs")
        elseif line_index == 4 then
          return explanation_tip("needs", "needs")
        elseif line_index == 6 then
          return class_tip("pop_housing", place_id, "pop", pop_index, "pop_housing")
        else
          return class_tip("pop_crowding", place_id, "pop", pop_index, "pop_crowding")
        end
      end
      return summary_tip(x, y)
    end,
    draw = function(group, x, y, width)
      row_width = width // P
      local result = view()
      local dx, dy = x // P, y // P
      local ink = open and theme.kind("place").title_text or theme.colour("text")
      put_quad(result, group, dx, dy, width // P, 13, open and theme.kind("place").outline or theme.colour("band"))
      put_quad(result, group, dx, dy, 2, 13, type_colour(pop.kind))
      local type_ink = open and theme.row_ink(type_colour(pop.kind), theme.kind("place").outline, ink) or type_colour(pop.kind)
      draw_icon(result, group, "pops", dx + 4, dy + 1, type_ink)
      local number_colour = open and theme.kind("place").highlight_number or theme.colour("number")
      if open then
        local term_ink = theme.row_ink(theme.colour("term"), theme.kind("place").outline, ink)
        put_text(result, group, dx + 16, dy + 1, pop.kind .. " " .. pop.culture, term_ink, nil, number_colour)
      else
        put_term_text(result, group, dx + 16, dy + 1, {term(pop.kind .. " " .. pop.culture)})
      end
      put_text(result, group, dx + width // P - 64, dy + 1, tostring(pop.workers + pop.dependents), ink, "right", number_colour)
      put_text(result, group, dx + width // P - 32, dy + 1, pop.food_days .. "d", ink, "right", number_colour)
      put_text(result, group, dx + width // P - 3, dy + 1, (pop.satisfaction >= 0 and "+" or "") .. pop.satisfaction, ink, "right", number_colour)
      if open then
        for index, line in ipairs(lines) do
          local y = dy + 13 + (index - 1) * 12
          if index == 1 then
            put_term_text(result, group, dx + 16, y,
              {term(pop.culture), {text = " · ", colour = theme.colour("text")}, term(pop.religion)})
          elseif index == 4 then
            put_term_text(result, group, dx + 16, y,
              {term("needs "), {text = percent(pop.necessities) .. " " .. percent(pop.niceties) .. " " .. percent(pop.luxuries)}})
          elseif index == 7 then
            local crowding_over = pop.crowding_pm ~= false and pop.crowding_pm ~= nil and pop.crowding_pm > 1000
            put_text(result, group, dx + 16, y, line, theme.colour("text"), nil,
              crowding_over and theme.colour("over_limit") or nil)
          else
            put_text(result, group, dx + 16, y, line, theme.colour("text"))
          end
        end
      end
      return result
    end,
  }
end

local function building_row(building, _, place_id, building_index)
  local layout
  local function arrange(width)
    local wage = building.has_methods and "offers" or milli(building.wage)
    local wage_left = width - 3 - font.layout(wage).width
    local count = building.filled .. "/" .. (building.filled + building.open)
    local count_width = font.layout(count).width
    local method = #building.methods > 0
    local positions = true
    local name = (building.abandoned and "abandoned " or (building.paused and "paused " or "")) .. building.kind
    local function needed() return 16 + font.layout(name).width + (method and 13 or 0) + (positions and count_width + 4 or 0) + 4 end
    if needed() > wage_left then positions = false end
    if needed() > wage_left then method = false end
    if needed() > wage_left then
      while utf8.len(name) > 0 and 16 + font.layout(name .. "…").width + 4 > wage_left do
        name = name:sub(1, utf8.offset(name, -1) - 1)
      end
      name = name .. "…"
    end
    local method_x = 16 + font.layout(name).width + 4
    return {name = name, method = method, method_x = method_x, positions = positions, count = count,
      count_x = wage_left - 4, wage = wage, wage_x = width - 3}
  end
  return {
    height = 13,
    tooltip = function(x, y)
      if layout == nil or y < 1 or y >= 13 then return nil end
      if x < 16 + font.layout(layout.name).width then return explanation_tip(building.kind, "kind") end
      if layout.method and within(x, y, layout.method_x, 1, layout.method_x + 9, 12) then return card_method_tip(building) end
      if layout.positions and within(x, y, layout.count_x - font.layout(layout.count).width, 1, layout.count_x, 12) then
        return class_tip("staff", place_id, "building", building_index, "staff")
      end
      if within(x, y, layout.wage_x - font.layout(layout.wage).width, 1, layout.wage_x, 12) then
        return class_tip("wage", place_id, "building", building_index, "wage")
      end
      return nil
    end,
    draw = function(group, x, y, width)
      layout = arrange(width // P)
      local result, dx, dy = view(), x // P, y // P
      put_quad(result, group, dx, dy, width // P, 13, theme.colour("band"))
      local icon = building_icon(building.kind)
      if icon ~= nil then draw_icon(result, group, icon, dx + 3, dy + 1, theme.colour("text")) end
      put_term_text(result, group, dx + 16, dy + 1, {term(layout.name)})
      if layout.method then
        local id = #building.methods > 1 and "split" or building.methods[1].id
        local quad = render.create_quad{texture = ui.texture(method_texture(id)), x = (dx + layout.method_x) * P,
          y = (dy + 1) * P, width = 9 * P, height = 11 * P, group = group}
        result:add({destroy = function() render.destroy_quad(quad) end})
      end
      if layout.positions then put_text(result, group, dx + layout.count_x, dy + 1, layout.count, theme.colour("text"), "right") end
      put_text(result, group, dx + layout.wage_x, dy + 1, layout.wage, theme.colour("text"), "right")
      return result
    end,
  }
end

-- Fits a market name before its price column, retaining whole UTF-8 characters.
local function clip_column(value, width)
  if width <= 0 then return "" end
  if font.layout(value).width <= width then return value end
  if font.layout("..").width > width then return "" end
  local count = utf8.len(value) or 0
  while count > 0 do
    local boundary = utf8.offset(value, count + 1)
    local prefix = boundary ~= nil and value:sub(1, boundary - 1) or value
    local candidate = prefix .. ".."
    if font.layout(candidate).width <= width then return candidate end
    count = count - 1
  end
  return ".."
end

local function temperature_line(place)
  if place.temperature == nil then return nil end
  local months = {"January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"}
  local function degrees(value)
    local sign = value < 0 and "-" or ""
    value = math.abs(value)
    return sign .. (value // 10) .. "." .. (value % 10) .. "°"
  end
  local value = place.temperature
  return "temperature " .. degrees(value.mean) .. " · " .. months[value.month] .. " " .. degrees(value.expected)
end

local function wind_line(place)
  if place.wind.knots == 0 then return "wind calm" end
  return "wind " .. place.wind.knots .. " kn " .. place.wind.direction
end

-- The function a graph icon calls: `handler(subject, graph)` with a subject `{kind, id}` and a graph key (CK-441). Without one the tab has no icons.
local graph_handler = nil

local function location_lines(place, width)
  if place.state == "abandoned" then
    local lines = {(tostring(place.kind):gsub("_", " ")), {tooltip_parts = {{text = "abandoned", term = "abandoned"}}, place = place.id}}
    if place.temperature ~= nil then
      lines[#lines + 1] = {tooltip_parts = {{text = "temperature", term = "temperature"}, {text = temperature_line(place):sub(12)}}, place = place.id}
    end
    if place.wind ~= nil then
      lines[#lines + 1] = {tooltip_parts = {{text = "wind", term = "wind"}, {text = wind_line(place):sub(5)}}, place = place.id}
    end
    return lines
  end
  local content_width = math.max(1, (width or 412) // P)
  local lines = {}
  local right = content_width - 1
  local function source(term_key, class_key)
    return function(_, _, cell)
      local target = cell == 1 and term_key or class_key
      if target == nil then return nil end
      if cell == 1 then return tooltip.explanation(target), tostring(cell) end
      return tooltip.content(target, {place = place.id}), tostring(cell)
    end
  end
  local function heading(word, key)
    local x = 1 + font.layout(word).width + 3
    lines[#lines + 1] = {cells = {{text = word, x = 1, align = "left", colour = theme.kind("place").divider}},
      bar = {x = x, width = math.max(3, right - x), mode = "rule", colour = theme.kind("place").outline},
      tooltip = key and source(key, nil) or nil}
  end
  local function single(label, key, value, class, over)
    lines[#lines + 1] = {cells = {{text = label, x = 1, align = "left", colour = theme.colour("term")},
      {text = value, x = right, align = "right", over_limit = over}}, tooltip = source(key, class)}
  end
  local function figure(label, key, value, capacity, unit, class)
    local label_width = font.layout(label).width
    local ratio = tostring(value) .. "/" .. tostring(capacity) .. (unit and " " .. unit or "")
    local ratio_width = font.layout(ratio).width
    local start = math.max(72, 1 + label_width + 4)
    local bar_width = math.min(40, right - ratio_width - 4 - start)
    local over = value > capacity
    local fill = capacity == 0 and (value > 0 and 1 or 0) or math.min(1, value / capacity)
    local function bar(x, width)
      return {x = x, width = math.max(3, width), mode = "capacity", value = fill}
    end
    local function label_cell() return {text = label, x = 1, align = "left", colour = theme.colour("term")} end
    local function number_cell(value_text, font_name, is_value)
      return {text = value_text, x = right, align = "right", font = font_name, over_limit = is_value and over or false}
    end
    local function pair_cells()
      local suffix = "/" .. tostring(capacity) .. (unit and " " .. unit or "")
      return {text = tostring(value), x = right - font.layout(suffix).width, align = "right", over_limit = over},
        number_cell(suffix, nil, false)
    end
    if bar_width >= 12 then
      local value_cell, suffix_cell = pair_cells()
      lines[#lines + 1] = {cells = {label_cell(), value_cell, suffix_cell}, bar = bar(start, bar_width), tooltip = source(key, class)}
    elseif 1 + label_width + 4 + ratio_width <= right then
      local value_cell, suffix_cell = pair_cells()
      lines[#lines + 1] = {cells = {label_cell(), value_cell, suffix_cell}, tooltip = source(key, class)}
      lines[#lines + 1] = {cells = {}, bar = bar(1, right - 1), tooltip = source(nil, class)}
    else
      lines[#lines + 1] = {cells = {label_cell()}, tooltip = source(key, nil)}
      local parts = {ratio}
      if ratio_width > right - 1 then parts = {tostring(value) .. "/", tostring(capacity) .. (unit and " " .. unit or "")} end
      for index, part in ipairs(parts) do
        local font_name = font.layout(part).width > right - 1 and "small" or nil
        if font.layout(part, font_name or "standard").width > right - 1 then
          local count = utf8.len(part)
          while count > 0 and font.layout(part:sub(1, (utf8.offset(part, count + 1) or (#part + 1)) - 1) .. "…", font_name).width > right - 1 do
            count = count - 1
          end
          part = (count > 0 and part:sub(1, (utf8.offset(part, count + 1) or (#part + 1)) - 1) or "") .. "…"
        end
        local cells
        if #parts == 1 then cells = {pair_cells()} else cells = {number_cell(part, font_name, index == 1)} end
        lines[#lines + 1] = {cells = cells, tooltip = function() return tooltip.content(class, {place = place.id}), "figure" end}
      end
      lines[#lines + 1] = {cells = {}, bar = bar(1, right - 1), tooltip = source(nil, class)}
    end
  end
  heading("people", "population")
  local births, deaths = 0, 0
  for _, pop in ipairs(place.pops or {}) do births, deaths = births + (pop.births or 0), deaths + (pop.deaths or 0) end
  -- A figure with a graph (CK-441): its words, its tooltip target and the graph its icon opens.
  local people = {{place.workers + place.dependents, "people", "population", "population", "people"},
    {place.food_days, "food days", "food_days", "place_food", "food_days"}, {place.workers, "workers", "worker", "population", "people"},
    {place.dependents, "dependents", "dependent", "population", "people"}, {births, "births", nil, nil, "births_deaths"},
    {deaths, "deaths", nil, nil, "births_deaths"}}
  local graphs = graph_handler ~= nil
  -- An icon sits 3 UI pixels after its word and is 9 wide.
  local function people_width(entry) return font.layout(tostring(entry[1]) .. " " .. entry[2]).width + (graphs and 12 or 0) end
  local widest = 0
  for index = 1, #people, 2 do widest = math.max(widest, people_width(people[index]) + people_width(people[index + 1])) end
  local columns = widest + 8 <= right - 1
  local function people_row(first, second)
    local cells, targets, hotspots = {}, {}, {}
    local function add(entry, x)
      local number = tostring(entry[1]) .. " "
      cells[#cells + 1] = {text = number, x = x, align = "left"}; targets[#targets + 1] = {class = entry[4]}
      cells[#cells + 1] = {text = entry[2], x = x + font.layout(number).width, align = "left", colour = theme.colour("term")}
      targets[#targets + 1] = {term = entry[3]}
      if graphs then
        local graph = entry[5]
        hotspots[#hotspots + 1] = {x = x + font.layout(number .. entry[2]).width + 3, icon = "graph",
          on_click = function() graph_handler({kind = "place", id = place.id}, graph) end}
      end
    end
    add(first, 1)
    if second ~= nil then add(second, right - people_width(second)) end
    lines[#lines + 1] = {cells = cells, hotspots = #hotspots > 0 and hotspots or nil, tooltip = function(_, _, cell)
      local target = targets[cell]
      if target == nil then return nil end
      if target.term then return tooltip.explanation(target.term), tostring(cell) end
      if target.class == nil then return nil end
      return tooltip.content(target.class, {place = place.id}), tostring(cell)
    end}
  end
  if columns then for index = 1, #people, 2 do people_row(people[index], people[index + 1]) end
  else for _, entry in ipairs(people) do people_row(entry) end end
  heading("housing", "housing")
  figure("basic", "basic_housing", place.basic_housing_occupants or 0, place.basic_housing_capacity or 0, nil, "housing")
  figure("makeshift", "makeshift_housing", place.makeshift_housing_occupants or 0, place.makeshift_housing_capacity or 0, nil, "housing")
  single("crowding", "crowding", percent(place.crowding_pm or 0), "crowding", (place.crowding_pm or 0) > 1000)
  heading("land and storage")
  figure("building land", "building_land", place.footprint_m2, math.min(place.prepared_m2, place.max_buildable_m2), "m²", "building_land")
  figure("storage", "storage", place.storage_used_l, place.storage_capacity_l, "L", "storage")
  single("lost to overflow", "overflow", tostring(place.overflow_lost), "overflow", place.overflow_lost > 0)
  heading("weather")
  if place.temperature ~= nil then
    lines[#lines + 1] = {tooltip_parts = {{text = "temperature", term = "temperature"}, {text = temperature_line(place):sub(12)}}, place = place.id}
  end
  if place.wind ~= nil then
    lines[#lines + 1] = {tooltip_parts = {{text = "wind", term = "wind"}, {text = wind_line(place):sub(5)}}, place = place.id}
  end
  heading("market")
  local price_right = content_width - 82
  local base_right = content_width - 56
  local bar_x = content_width - 48
  local widest_price = 0
  for _, price in ipairs(place.prices) do
    widest_price = math.max(widest_price, font.layout(milli(price.price_milli)).width)
  end
  local name_width = math.max(0, price_right - widest_price - 4 - 16 - (graph_handler ~= nil and 12 or 0))
  local header = {cells = {
    {text = "good", x = 16, align = "left", colour = theme.colour("term"), identity = true},
    {text = "price", x = price_right, align = "right", colour = theme.colour("term"), identity = true},
    {text = "base", x = base_right, align = "right", colour = theme.colour("term"), identity = true},
    {text = "buy/sell", x = bar_x, align = "left", colour = theme.colour("dim")},
  }}
  header.tooltip = function(_, _, cell)
    local explanation = ({[1] = "good", [2] = "price", [3] = "base_price"})[cell]
    if explanation == nil then return nil end
    return tooltip.explanation(explanation), tostring(cell)
  end
  lines[#lines + 1] = header
  for _, price in ipairs(place.prices) do
    local entry = glossary.entry(price.good)
    local name = entry ~= nil and entry.title or good_name(price.good)
    local name_colour = entry ~= nil and theme.colour("term") or theme.colour("text")
    local value
    if price.bought ~= nil and price.offered ~= nil then
      local largest = math.max(price.bought, price.offered)
      value = largest > 0 and (price.offered - price.bought) / largest or 0
    end
    local shown_name = clip_column(name, name_width)
    local row = {
      -- A graph icon after the good's name (CK-441).
      hotspots = graph_handler ~= nil and {{x = 16 + font.layout(shown_name).width + 3, icon = "graph",
        on_click = function() graph_handler({kind = "place", id = place.id}, "price:" .. price.good) end}} or nil,
      -- What the list bar sorts and filters by (CK-296): the name as shown before it is clipped, and the good's tags.
      sort = {name = name, price = price.price_milli, base_price = price.base, buy_sell = value},
      filter = {tag = price.tags},
      cells = {
        {text = shown_name, x = 16, align = "left", colour = name_colour, identity = entry ~= nil},
        {text = milli(price.price_milli), x = price_right, align = "right"},
        {text = tostring(price.base), x = base_right, align = "right"},
      },
      bar = {x = bar_x, width = 46, value = value},
      tooltip = function(_, _, cell)
        if cell == "icon" or cell == 1 then
          if entry ~= nil then return tooltip.explanation(price.good), tostring(cell) end
        elseif cell == 2 or cell == "bar" then
          return tooltip.content("price", {place = place.id, good = price.good}), tostring(cell)
        elseif cell == 3 then
          return tooltip.explanation("base_price"), tostring(cell)
        end
        return nil
      end,
    }
    row.icon = {image = good_icon(price.good)}
    lines[#lines + 1] = row
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
    lines[#lines + 1] = (building.abandoned and "abandoned " or (building.paused and "paused " or "")) .. building.kind
    if building.has_methods then
      local line = "  units: " .. building.units
      if building.free_units > 0 then line = line .. ", free: " .. building.free_units end
      if #building.methods > 0 then
        local methods = {}
        for _, method in ipairs(building.methods) do
          methods[#methods + 1] = method_label(method)
        end
        line = line .. ", levels: " .. table.concat(methods, ", ")
      end
      lines[#lines + 1] = line
    else
      lines[#lines + 1] = "  levels: " .. building.levels
    end
    lines[#lines + 1] = "  positions: " .. building.filled .. " filled, " .. building.open .. " open"
    lines[#lines + 1] = "  housing occupants: " .. (building.housing_occupants or 0)
    for _, herd in ipairs(building.herds) do
      lines[#lines + 1] = "  " .. herd.species .. ": " .. herd.head .. " / " .. herd.maximum
    end
    lines[#lines + 1] = "  store: " .. building.store_used_l .. " / " .. building.store_capacity_l .. " L"
    if building.has_methods then
      for _, offer in ipairs(building.offers or {}) do
        lines[#lines + 1] = "  " .. offer.id:gsub("_", " ") .. " offer: " .. milli(offer.offer_milli)
      end
    else
      lines[#lines + 1] = "  wage: " .. building.wage
    end
  end
  return lines
end

local FORMATTERS = {
  location = location_lines,
  pops = pops_lines,
  buildings = buildings_lines,
}

-- The lines of tab `key` (`"location"`, `"pops"` or `"buildings"`) for `place`, a table `game.place` returned.
-- Sets the function the graph icons call, `handler(subject, graph)` with a subject `{kind, id}` and a graph key; nil takes the icons away.
function location.set_graph_handler(handler)
  graph_handler = handler
end

function location.lines(key, place, width)
  local format = FORMATTERS[key]
  if format == nil then
    error("location.lines: `" .. tostring(key) .. "` is not a tab", 2)
  end
  if key == "location" then return format(place, width) end
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
  local source_cache = {}
  local function part_source(part)
    if part.term == nil and part.class == nil then return nil end
    local kind, value = part.term ~= nil and "term" or "class", part.term or part.class
    local key = kind .. ":" .. value .. ":" .. tostring(part.over_limit == true)
    local source = source_cache[key]
    if source ~= nil then return source end
    if part.term ~= nil then source = {term = part.term, over_limit = part.over_limit == true}
    elseif part.class ~= nil then source = {class = part.class, over_limit = part.over_limit == true}
    else return nil end
    source_cache[key] = source
    return source
  end
  for _, line in ipairs(lines) do
    if type(line) == "table" and line.tooltip_parts ~= nil then
      local place_id = line.place
      local current = {cells = {}, tooltip_place = line.place}
      local current_width = 0
      local function add_cell(text_value, source)
        if #current.cells > 0 and current.cells[#current.cells].source == source then
          current.cells[#current.cells].text = current.cells[#current.cells].text .. text_value
        else
          current.cells[#current.cells + 1] = {text = text_value, x = current_width + 1, align = "left", source = source}
        end
        current_width = current_width + font.layout(text_value).width
      end
      local function finish()
        if #current.cells == 0 then return end
        local sources = {}
        for index, cell in ipairs(current.cells) do sources[index] = cell.source end
        current.tooltip = function(_, _, cell)
          local source = sources[cell]
          if source == nil then return nil end
          if source.term ~= nil then return tooltip.explanation(source.term), tostring(cell) end
          return tooltip.content(source.class, {place = place_id}), tostring(cell)
        end
        for _, cell in ipairs(current.cells) do
          cell.colour = cell.source ~= nil and cell.source.term ~= nil and theme.colour("term") or theme.colour("text")
          cell.over_limit = cell.source ~= nil and cell.source.over_limit == true
          cell.source = nil
        end
        rows[#rows + 1] = current
        current = {cells = {}, tooltip_place = line.place}
        current_width = 0
      end
      local full_text = ""
      for _, part in ipairs(line.tooltip_parts) do
        full_text = full_text .. part.text
      end
      if font.layout(full_text).width <= limit then
        for _, part in ipairs(line.tooltip_parts) do
          add_cell(part.text, part_source(part))
        end
      else
        local indent = full_text:match("^ *")
        local continued = indent .. "  "
        local words = {}
        local ranges = {}
        local offset = 1
        for _, part in ipairs(line.tooltip_parts) do
          local source = part_source(part)
          if #part.text > 0 then
            ranges[#ranges + 1] = {first = offset, last = offset + #part.text - 1, source = source}
            offset = offset + #part.text
          end
        end
        local at = 1
        while true do
          local first, last = full_text:find("%S+", at)
          if first == nil then break end
          words[#words + 1] = {text = full_text:sub(first, last), first = first, last = last}
          at = last + 1
        end
        local function add_sourced_slice(item, first_byte, last_byte, separator)
          local position = first_byte
          while position <= last_byte do
            local absolute = item.first + position - 1
            local source, end_at = nil, last_byte
            for _, range in ipairs(ranges) do
              if absolute >= range.first and absolute <= range.last then
                source = range.source
                end_at = math.min(last_byte, range.last - item.first + 1)
                break
              elseif range.first > absolute then
                end_at = math.min(last_byte, range.first - item.first)
                break
              end
            end
            local prefix = position == first_byte and separator or ""
            add_cell(prefix .. item.text:sub(position, end_at), source)
            position = end_at + 1
          end
        end
        local row_text = ""
        local first_wrapped_row = true
        for _, item in ipairs(words) do
          local word = item.text
          local lead = #row_text == 0 and (first_wrapped_row and indent or continued) or ""
          local separator = #row_text == 0 and "" or " "
          local candidate = row_text == "" and (lead .. word) or (row_text .. separator .. word)
          if font.layout(candidate).width > limit and row_text ~= "" then
            finish()
            first_wrapped_row = false
            row_text = ""
            lead = continued
            separator = ""
            candidate = lead .. word
          end
          if lead ~= "" then add_cell(lead, nil) end
          row_text = lead .. row_text
          -- Preserve wrap's UTF-8-safe cut for a token wider than the available row.
          local word_offset = 1
          while font.layout(candidate).width > limit and utf8.len(word) > 1 do
            local cut = utf8.len(word) - 1
            while cut > 1 and font.layout(lead .. word:sub(1, utf8.offset(word, cut + 1) - 1)).width > limit do
              cut = cut - 1
            end
            local at = utf8.offset(word, cut + 1)
            local piece = word:sub(1, at - 1)
            add_sourced_slice(item, word_offset, word_offset + #piece - 1, separator)
            row_text = row_text .. separator .. piece
            finish()
            first_wrapped_row = false
            row_text = ""
            word = word:sub(at)
            lead = continued
            add_cell(lead, nil)
            row_text = lead
            separator = ""
            candidate = lead .. word
            word_offset = word_offset + #piece
          end
          add_sourced_slice(item, word_offset, word_offset + #word - 1, separator)
          row_text = row_text .. separator .. word
        end
      end
      finish()
    elseif type(line) == "table" then
      rows[#rows + 1] = line
    else
      for _, row in ipairs(wrap(line, limit)) do
        rows[#rows + 1] = row
      end
    end
  end
  return rows
end

-- A panel over the windows of `spec.view` (a table with the window's current `width` and `height` in points, kept up to date by the
-- caller) that sits `spec.top()` points from the top of the screen (below the time bar, say). `spec.on_close` is called when the
-- window closes. `spec.on_go(id, revision)` is called when the title bar's go-to button is clicked, with the place showing and the revision
-- the panel holds.
local render

function location.new(spec)
  local self = setmetatable({}, location)
  self.view = spec.view
  self.on_close = spec.on_close
  self.on_go = spec.on_go
  self.on_detail = spec.on_detail
  self.top = spec.top or function() return 0 end
  self.window = nil
  self.list = nil
  self.cards = nil
  self.selector = nil
  self.bar, self.bar_key = nil, nil
  self.tabs = nil
  self.id = nil
  self.revision = nil
  self.open_pops = nil
  self.open_buildings = nil
  self.key = TABS[1].key
  self.list_look = list_look.get()
  self.theme_outline = theme.kind("place").outline
  theme.watch(self, function(panel)
    local outline = theme.kind("place").outline
    if panel.theme_outline ~= outline then
      panel.theme_outline = outline
      if panel.window ~= nil and not panel.window.closed and panel.key == "location" then
        render(panel, true)
      end
    end
  end)
  list_look.subscribe(function(value)
    self.list_look = value
    if self.selector ~= nil then
      for key, control in pairs(self.selector.buttons) do control:set_selected(key == value) end
    end
    if self.window ~= nil and not self.window.closed then render(self, false) end
  end)
  return self
end

local function open(self)
  return self.window ~= nil and not self.window.closed
end

-- The narrowest the panel gets, in points: what holds its three tabs (with their tiny labels), the close button and the frame.
function location.ledger_min_width()
  -- Also what holds the market's columns beside the scroll bar of its list: they end 48 pixels from the row's right edge (the balance
  -- bar is 46 wide from there), and the bar takes 5 pixels of the content's width (CK-299).
  local columns = (48 + scrollbar.RESERVE // P) * P + 6 * P
  return math.max(windows.min_outer_width(tabs.reserved(TABS), false, windows.title_button_room(GO_TO_ICON)), columns)
end

function location.min_width()
  return math.max(144 * P, location.ledger_min_width())
end

-- The outer size of the panel and where it sits: the left third of the screen, from `top` to the bottom edge. Where the third is
-- narrower than `min_width` (a very narrow window or a large scale) the panel is that wide, and may pass the window's edge.
local function geometry(self)
  local top = ui.snap(self.top())
  local width = math.max(ui.snap_size(self.view.width // 3), ui.snap(location.min_width()))
  local height = ui.snap_size(math.max(self.view.height - top, 8 * P))
  return top, width, height
end

-- The left slot below `top` points for a window `view` points wide and tall: its top edge, width and height. The character screen
-- (CK-375) draws over it, where the location panel is.
function location.slot(view, top)
  return geometry({view = view, top = function() return top end})
end

-- The place's view at the revision this panel knows, or nil with the reason when it has none.
local function fetch(self)
  if self.revision == nil or self.revision == 0 then
    return nil, "economy: not published yet"
  end
  local place, reason = game.place(self.id, self.revision)
  local weather = self.weather and self.weather[self.id]
  if place ~= nil and weather ~= nil then
    if place.temperature ~= nil then place.temperature.mean = weather.temperature end
    place.wind = weather.wind
  end
  return place, reason
end

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
    if not list_look.set(key) then return end
    if self_owner.cards ~= nil and not self_owner.cards.closed then self_owner.cards:set_pos(0) end
  end
  for _, key in ipairs({"cards", "rows"}) do
    selector.buttons[key] = button.new{
      x = 0, y = 0, icons = {key}, icon = key, kind = "place", style = "panel", group = self.window.content,
      tooltip = key == "cards" and "cards" or "list",
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
    if self.closed then return end
    self.closed = true
    if self_owner.window ~= nil then self_owner.window:detach(self) end
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


-- What the bars of the location panel sort and filter by (CK-296; labels as the owner approved them), by tab.
local BARS = {
  location = {
    sorts = {
      {key = "name", label = "name", first = "ascending"},
      {key = "price", label = "price", first = "descending"},
      {key = "base_price", label = "base price", first = "descending"},
      {key = "buy_sell", label = "buy/sell", first = "descending"},
    },
    groups = {{key = "tag", label = "tag"}},
  },
  pops = {
    sorts = {
      {key = "type", label = "type", first = "ascending"},
      {key = "people", label = "people", first = "descending"},
      {key = "food", label = "food", first = "descending"},
      {key = "mood", label = "mood", first = "descending"},
    },
    groups = {{key = "type", label = "type"}},
  },
  buildings = {
    sorts = {
      {key = "kind", label = "kind", first = "ascending"},
      {key = "positions", label = "positions", first = "descending"},
      {key = "wage", label = "highest offer", first = "descending"},
    },
    groups = {{key = "kind", label = "kind"}},
  },
}

-- The pops or buildings of `place` as the rows the tab's bar takes: each keeps the item and its index in `place.pops` or
-- `place.buildings`, which every tooltip and the open row name, wherever the bar puts it.
local function bar_rows(key, place)
  local rows = {}
  if place == nil then return rows end
  for index, item in ipairs(key == "pops" and place.pops or place.buildings) do
    local row = {item = item, index = index}
    if key == "pops" then
      row.sort = {type = item.kind, people = item.workers + item.dependents, food = item.food_days, mood = item.satisfaction}
      row.filter = {type = item.kind}
    else
      local highest = item.has_methods and 0 or item.wage
      for _, offer in ipairs(item.offers or {}) do highest = math.max(highest, offer.offer_milli) end
      row.sort = {kind = item.kind, positions = item.filled, wage = highest}
      row.filter = {kind = item.kind}
    end
    rows[#rows + 1] = row
  end
  return rows
end

-- The tab's bar. Its choices start again with a new bar (the panel opens, the tab changes, another place is shown) and stay while it
-- lives (a new revision, a new window size, the same place shown again, the look changing between cards and rows).
local function drop_bar(self)
  if self.bar ~= nil then self.bar:destroy() end
  self.bar, self.bar_key = nil, nil
end

local function ensure_bar(self)
  if self.bar ~= nil and self.bar_key == self.key and not self.bar.closed then return end
  drop_bar(self)
  local spec = BARS[self.key]
  self.bar_key = self.key
  self.bar = listbar.new{
    window = self.window,
    sorts = spec.sorts,
    groups = spec.groups,
    -- A change shows the pops or buildings from their top; the location tab's list keeps its scroll, cut to its new length.
    on_change = function() render(self, self.key == "location") end,
  }
end

local function row_items(self, key, place, shown)
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
    for _, entry in ipairs(shown) do
      local pop, index = entry.item, entry.index
      local item = pop_row(pop, false, place.id, index)
      local world = self.world
      item.on_click = function() pointer.defer(function()
        if self.world == world and self.on_detail ~= nil then self.on_detail("pop", place.id, index, world) end
      end) end
      items[#items + 1] = item
    end
  else
    header({{text = "building", x = 16}, {text = "positions", right = 44}, {text = "pay", right = 3}})
    for _, entry in ipairs(shown) do
      local building, index = entry.item, entry.index
      local item = building_row(building, false, place.id, index)
      local world = self.world
      item.on_click = function() pointer.defer(function()
        if self.world == world and self.on_detail ~= nil then self.on_detail("building", place.id, index, world) end
      end) end
      items[#items + 1] = item
    end
  end
  return items
end

local function card_items(self, key, place, reason, shown)
  if place == nil then
    return {{height = 13, draw = function(group) return text_view(group, 1, 1, reason or "economy: not published yet", theme.colour("text")) end}}
  end
  local items, source = {}, key == "pops" and place.pops or place.buildings
  if #source == 0 then
    return {{height = 13, draw = function(group)
      return text_view(group, 1, 1, key == "pops" and "no pops" or "no buildings", theme.colour("dim"))
    end}}
  end
  for _, entry in ipairs(shown) do
    local item, index = entry.item, entry.index
    local card = key == "pops" and pop_card(item, place.id, index) or building_card(item, place.id, index)
    local world = self.world
    card.on_click = function() pointer.defer(function()
      if self.world == world and self.on_detail ~= nil then self.on_detail(key == "pops" and "pop" or "building", place.id, index, world) end
    end) end
    items[#items + 1] = card
  end
  return items
end

-- Makes the list show the current tab's rows, keeping its scroll when `keep_scroll`.
render = function(self, keep_scroll)
  if not open(self) then
    return
  end
  local place, reason = fetch(self)
  if (self.key == "pops" or self.key == "buildings") and not (place ~= nil and place.state == "abandoned") then
    if self.list ~= nil and not self.list.closed then self.list:destroy(); self.list = nil end
    ensure_selector(self)
    ensure_bar(self)
    local strip_top = math.max(0, self.window.content_height - 19 * P)
    local shown = self.bar:view(bar_rows(self.key, place))
    local items = self.list_look == "cards" and card_items(self, self.key, place, reason, shown)
      or row_items(self, self.key, place or {pops = {}, buildings = {}}, shown)
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
  ensure_bar(self)
  -- The rows of the list laid out for a row `width` points wide. Only the goods rows go through the bar; the lines of figures and the
  -- market's header stay where they are, above them.
  local function laid_out(width)
    local lines = place ~= nil and (place.state == "abandoned" and location_lines(place, width) or location.lines(self.key, place, width)) or {reason}
    local fixed, goods = {}, {}
    for _, row in ipairs(location.rows(lines, width)) do
      if type(row) == "table" and row.sort ~= nil then goods[#goods + 1] = row else fixed[#fixed + 1] = row end
    end
    local rows = fixed
    for _, row in ipairs(self.bar:view(goods)) do rows[#rows + 1] = row end
    return rows
  end
  -- A list shows at most `MAX_ROWS` rows; a taller window leaves the rest of its content empty.
  local count = math.min(lists.MAX_ROWS, math.max(1, (self.window.content_height - listbar.HEIGHT) // lists.ROW))
  if self.list ~= nil and not self.list.closed then count = self.list.rows end
  -- The rows' number depends on the width (the lines of figures wrap): lay out at the full width, and when that does not fit the view
  -- (so the list has its scroll bar) lay out again at the row width the list gives for that many rows. Narrowing a wrapped text never
  -- makes it shorter, so the second layout overflows too.
  local full = self.window.content_width
  local rows = laid_out(full)
  local row_width = lists.row_width(full, count, #rows)
  if row_width ~= full then rows = laid_out(row_width) end
  if self.list ~= nil and not self.list.closed then
    self.list:set_items(rows)
    if not keep_scroll then
      self.list:set_pos(0)
    end
  else
    self.list = lists.new{
      window = self.window,
      x = 0,
      y = 0,
      width = self.window.content_width,
      rows = count,
      items = rows,
      tooltips = true,
      numbers = true,
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
-- place in it, on the tab that is showing. With a `revision` the panel reads the place under it, the revision the caller named (a go-to
-- opens the window for the revision it went under); the next revision `game_time` carries refreshes the panel as it always does.
function location:show(selection, tab, revision)
  if revision ~= nil and revision ~= 0 then self:set_revision(revision) end
  if tab ~= nil then
    if FORMATTERS[tab] == nil then error("location:show: `" .. tostring(tab) .. "` is not a tab", 2) end
    self.key = tab
  end
  if self.id ~= nil and self.id ~= selection.id then
    self.open_pops, self.open_buildings = nil, nil
    drop_bar(self)
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
          self.bar, self.bar_key = nil, nil
          self.tabs = nil
        end
        if self.on_close ~= nil then
          self.on_close()
        end
      end,
    }
    self.window = created
    -- The name belongs to the first tab, not to a separate title beside it.
    created:set_title("")
    self.tabs = tabs.new{
      window = created,
      items = TABS,
      align = "left",
      selected = self.key,
      on_select = function(key)
        self.key = key
        render(self, false)
      end,
    }
    -- The title bar's go-to button: the camera goes to this place and zooms in; the window stays open, it is the place's own.
    created:add_title_button{
      icon = GO_TO_ICON,
      tooltip = "go to",
      on_click = function()
        if self.on_go ~= nil and self.id ~= nil then self.on_go(self.id, self.revision) end
      end,
    }
    update_location_tab(self, selection)
    render(self, false)
    return
  end
  update_location_tab(self, selection)
  self.window:raise()
  if tab ~= nil then
    self.tabs:select(tab)
  end
  render(self, false)
end

-- The place names show differently now (CK-423): the open panel takes the name the choice shows from the place's id, for its first tab (the
-- window's title) and for every list of the tab showing, keeping its scroll. No new revision and no tick is needed.
function location:relabel()
  if not open(self) or self.id == nil then return end
  update_location_tab(self, {id = self.id, name = place_names.name_of(self.id)})
  render(self, true)
end

-- The economy revision and hourly weather belong to the same time snapshot.
function location:set_revision(revision, weather, year, month, day, hour, world)
  price_revision = revision
  self.world = world or self.world
  local changed = revision ~= self.revision
  self.revision = revision
  if weather ~= nil then
    local key = table.concat({year or 0, month or 0, day or 0, hour or 0, world or 0}, ":")
    changed = changed or key ~= self.weather_key
    self.weather, self.weather_key = weather, key
  end
  if changed and open(self) then render(self, true) end
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
