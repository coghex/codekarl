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
local scrollbar = require("lib.scrollbar")
local tabs = require("lib.tabs")
local cards = require("lib.cards")
local listbar = require("lib.listbar")
local button = require("lib.button")
local frame = require("lib.frame")
local text = require("lib.text")
local theme = require("lib.theme")
local tooltip = require("lib.tooltip")

local P = ui.P

local location = {}
location.__index = location
local price_revision

local TABS = {
  {key = "location", label = "location", short = "loc", tiny = "l"},
  {key = "pops", label = "pops", short = "pop", tiny = "p"},
  {key = "buildings", label = "buildings", short = "bld", tiny = "b"},
}

-- This list is deliberately fixed: an unlisted future good keeps its readable name without an asynchronous file lookup.
local GOODS_WITH_ICONS = {
  "oats", "barley", "peas_beans", "vegetables", "apples", "dairy", "beef", "pork", "mutton", "fish", "seabirds",
  "ale", "mead", "wine", "honey", "herbs", "wool", "beeswax", "hides", "cattle", "pigs", "sheep",
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
    texture = ui.texture("ui/theme/icon_" .. name .. ".png"), x = x * P, y = y * P,
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

local function method_tip(building, x, y, origin_x, origin_y)
  local left, top = origin_x or 17, origin_y or 14
  for _, method in ipairs(building.methods) do
    local label = method.id .. " " .. method.levels
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
    height = #building.herds > 0 and 50 or 39,
    tooltip = function(x, y)
      if card_width == nil then return nil end
      if within(x, y, 5, 3, 14, 14) or within(x, y, 17, 3, 17 + font.layout(building.kind).width, 3 + font.layout(building.kind).height) then
        return explanation_tip(building.kind, "kind")
      end
      local wage_text = "wage " .. milli(building.wage)
      local wage_left = card_width - 5 - font.layout(wage_text).width
      if within(x, y, wage_left, 3, wage_left + font.layout("wage ").width, 3 + font.layout(wage_text).height) then
        return explanation_tip("wage", "wage_word")
      end
      if within(x, y, wage_left + font.layout("wage ").width, 3, card_width - 5, 3 + font.layout(wage_text).height) then
        return class_tip("wage", place_id, "building", building_index, "wage")
      end
      local method_content, method_part = method_tip(building, x, y)
      if method_content ~= nil then return method_content, method_part end
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
      local hx = 5
      for _, herd in ipairs(building.herds) do
        local count = herd.head .. "/" .. herd.maximum
        if x >= hx and x < hx + 9 and y >= 38 and y < 47 then
          return explanation_tip(herd.species, herd.species)
        end
        local count_left = hx + 11
        if within(x, y, count_left, 37, count_left + font.layout(count).width, 37 + font.layout(count).height) then
          return class_tip("herd", place_id, "building", building_index, herd.species, herd.species)
        end
        hx = hx + 11 + font.layout(count).width + 10
      end
      return nil
    end,
    draw = function(group, x, y, width)
      card_width = width // P
      local result = view()
      local dx, dy = x // P, y // P
      local colours = theme.kind("place")
      local w = width // P
      local height = (#building.herds > 0 and 50 or 39)
      result:add(frame.new{x = x, y = y, width = width, height = height * P, cut = 3, corners = "tl_br", fill = colours.fill, line = colours.outline, group = group})
      if icon ~= nil then draw_icon(result, group, icon, dx + 5, dy + 3, theme.colour("text")) end
      put_term_text(result, group, dx + 17, dy + 3, {term(building.kind)})
      put_term_text(result, group, dx + w - 5, dy + 3,
        {term("wage "), {text = milli(building.wage), colour = theme.colour("dim")}}, "right")
      local method_segments = {}
      for index, method in ipairs(building.methods) do
        if index > 1 then method_segments[#method_segments + 1] = {text = ", ", colour = theme.colour("dim")} end
        method_segments[#method_segments + 1] = term(method.id)
        method_segments[#method_segments + 1] = {text = " " .. method.levels, colour = theme.colour("dim")}
      end
      if #method_segments == 0 then
        method_segments[1] = {text = "storage", colour = theme.colour("dim")}
      end
      put_term_text(result, group, dx + 17, dy + 14, method_segments)
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
      local hx = dx + 5
      for _, herd in ipairs(building.herds) do
        local count = herd.head .. "/" .. herd.maximum
        draw_goods_icon(result, group, herd.species, hx, dy + 38)
        put_text(result, group, hx + 11, dy + 37, count, herd.head < herd.maximum and theme.colour("dim") or theme.colour("text"))
        hx = hx + 11 + font.layout(count).width + 10
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
      draw_icon(result, group, "pops", dx + 4, dy + 1, open and ink or type_colour(pop.kind))
      local number_colour = open and theme.kind("place").highlight_number or theme.colour("number")
      if open then
        put_text(result, group, dx + 16, dy + 1, pop.kind .. " " .. pop.culture, ink, nil, number_colour)
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

local function building_row(building, open, place_id, building_index)
  local lines = {methods_text(building), "store " .. building.store_used_l .. " / " .. building.store_capacity_l .. " L"}
  for _, herd in ipairs(building.herds) do lines[#lines + 1] = {herd = herd} end
  lines[#lines + 1] = "positions " .. building.filled .. " filled, " .. building.open .. " open"
  lines[#lines + 1] = "housing occupants " .. (building.housing_occupants or 0)
  local row_width
  return {
    height = open and (13 + 12 * #lines + 3) or 13,
    tooltip = function(x, y)
      if row_width == nil then return nil end
      if not open then
        if y < 1 or y >= 13 then return nil end
        if within(x, y, 3, 1, 12, 12) then return explanation_tip(building.kind, "kind") end
        local kind_right = 16 + font.layout(building.kind).width
        if within(x, y, 16, 1, kind_right, 1 + font.layout(building.kind).height) then
          return explanation_tip(building.kind, "kind")
        end
        local staff = building.filled .. "/" .. (building.filled + building.open)
        local staff_right = row_width - 44
        local staff_left = staff_right - font.layout(staff).width
        if within(x, y, staff_left, 1, staff_right, 1 + font.layout(staff).height) then
          return class_tip("staff", place_id, "building", building_index, "staff")
        end
        local wage = milli(building.wage)
        local wage_right, wage_left = row_width - 3, row_width - 3 - font.layout(wage).width
        if within(x, y, wage_left, 1, wage_right, 1 + font.layout(wage).height) then
          return class_tip("wage", place_id, "building", building_index, "wage")
        end
        return nil
      end
      if y < 13 then
        if within(x, y, 3, 1, 12, 12) or within(x, y, 16, 1, 16 + font.layout(building.kind).width, 1 + font.layout(building.kind).height) then
          return explanation_tip(building.kind, "kind")
        end
        local staff = building.filled .. "/" .. (building.filled + building.open)
        local staff_right, staff_left = row_width - 44, row_width - 44 - font.layout(staff).width
        if within(x, y, staff_left, 1, staff_right, 1 + font.layout(staff).height) then
          return class_tip("staff", place_id, "building", building_index, "staff")
        end
        local wage = milli(building.wage)
        local wage_right, wage_left = row_width - 3, row_width - 3 - font.layout(wage).width
        if within(x, y, wage_left, 1, wage_right, 1 + font.layout(wage).height) then
          return class_tip("wage", place_id, "building", building_index, "wage")
        end
        return nil
      end
      local line_index = math.floor((y - 13) / 12) + 1
      if line_index < 1 or line_index > #lines then return nil end
      local line = lines[line_index]
      if line_index == 1 then return method_tip(building, x, y, 16, 13)
      elseif line_index == 2 then
        local label_right = 16 + font.layout("storage").width
        if within(x, y, 16, 13 + (line_index - 1) * 12, label_right,
            13 + (line_index - 1) * 12 + font.layout("storage").height) then
          return explanation_tip("storage", "storage")
        end
        return class_tip("store", place_id, "building", building_index, "store")
      elseif type(line) == "table" then
        local herd = line.herd
        local herd_y = 13 + (line_index - 1) * 12
        if within(x, y, 16, herd_y, 25, herd_y + 10) then return explanation_tip(herd.species, herd.species) end
        local count_left = 28
        local count = herd.head .. " / " .. herd.maximum
        if within(x, y, count_left, herd_y, count_left + font.layout(count).width, herd_y + font.layout(count).height) then
          return class_tip("herd", place_id, "building", building_index, herd.species, herd.species)
        end
        return nil
      elseif line_index == #lines - 1 then
        local top = 13 + (line_index - 1) * 12
        local label_right = 16 + font.layout("positions").width
        if within(x, y, 16, top, label_right, top + font.layout("positions").height) then
          return explanation_tip("position", "positions")
        end
        return class_tip("staff", place_id, "building", building_index, "staff")
      else
        return explanation_tip("housing", "housing")
      end
    end,
    draw = function(group, x, y, width)
      row_width = width // P
      local result = view()
      local dx, dy = x // P, y // P
      local ink = open and theme.kind("place").title_text or theme.colour("text")
      put_quad(result, group, dx, dy, width // P, 13, open and theme.kind("place").outline or theme.colour("band"))
      local icon = building_icon(building.kind)
      if icon ~= nil then draw_icon(result, group, icon, dx + 3, dy + 1, ink) end
      local number_colour = open and theme.kind("place").highlight_number or theme.colour("number")
      if open then
        put_text(result, group, dx + 16, dy + 1, building.kind, ink, nil, number_colour)
      else
        put_term_text(result, group, dx + 16, dy + 1, {term(building.kind)})
      end
      put_text(result, group, dx + width // P - 44, dy + 1, building.filled .. "/" .. (building.filled + building.open), ink, "right", number_colour)
      put_text(result, group, dx + width // P - 3, dy + 1, milli(building.wage), ink, "right", number_colour)
      if open then
        for index, line in ipairs(lines) do
          local y = dy + 13 + (index - 1) * 12
          if type(line) == "table" then
            local herd = line.herd
            draw_goods_icon(result, group, herd.species, dx + 16, y + 1)
            put_text(result, group, dx + 28, y, herd.head .. " / " .. herd.maximum, theme.colour("text"))
          else
            if index == 1 then
              local segments = {}
              for method_index, method in ipairs(building.methods) do
                if method_index > 1 then segments[#segments + 1] = {text = ", ", colour = theme.colour("text")} end
                segments[#segments + 1] = term(method.id)
                segments[#segments + 1] = {text = " " .. method.levels, colour = theme.colour("text")}
              end
              if #segments == 0 then segments[1] = {text = "storage", colour = theme.colour("text")} end
              put_term_text(result, group, dx + 16, y, segments)
            elseif index == #lines then
              put_term_text(result, group, dx + 16, y,
                {term("housing "), {text = "occupants " .. tostring(building.housing_occupants or 0)}})
            else
              if index == 2 then
                put_term_text(result, group, dx + 16, y, {
                  term("storage"), {text = " "},
                  {text = tostring(building.store_used_l), number_colour = building.store_used_l > building.store_capacity_l
                    and theme.colour("over_limit") or nil},
                  {text = " / "}, {text = tostring(building.store_capacity_l)}, {text = " L"},
                })
              else
                if index == #lines - 1 then
                  put_term_text(result, group, dx + 16, y, {
                    term("positions"), {text = " " .. building.filled .. " filled, " .. building.open .. " open"},
                  })
                else
                  put_text(result, group, dx + 16, y, line, theme.colour("text"))
                end
              end
            end
          end
        end
      end
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

local function location_lines(place, width)
  local content_width = math.max(1, (width or 412) // P)
  local lines = {
    {tooltip_parts = {
      {text = "people: ", term = "population"}, {text = tostring(place.workers + place.dependents), class = "population"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "  workers: ", term = "worker"}, {text = tostring(place.workers), class = "population"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "  dependents: ", term = "dependent"}, {text = tostring(place.dependents), class = "population"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "housing: ", term = "housing"}, {text = "basic", term = "basic_housing"},
      {text = " ", term = "basic_housing"}, {text = tostring(place.basic_housing_occupants or 0), class = "housing",
        over_limit = (place.basic_housing_occupants or 0) > (place.basic_housing_capacity or 0)},
      {text = "/", class = "housing"}, {text = tostring(place.basic_housing_capacity or 0), class = "housing"},
      {text = ", "}, {text = "makeshift", term = "makeshift_housing"},
      {text = " ", term = "makeshift_housing"}, {text = tostring(place.makeshift_housing_occupants or 0), class = "housing",
        over_limit = (place.makeshift_housing_occupants or 0) > (place.makeshift_housing_capacity or 0)},
      {text = "/", class = "housing"}, {text = tostring(place.makeshift_housing_capacity or 0), class = "housing"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "crowding: ", term = "crowding"},
      {text = percent(place.crowding_pm or 0), class = "crowding", over_limit = (place.crowding_pm or 0) > 1000},
    }, place = place.id},
    {tooltip_parts = {
      {text = "food: ", term = "food_days"}, {text = place.food_days .. " days", class = "place_food"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "building land: ", term = "building_land"},
      {text = tostring(place.footprint_m2), class = "building_land",
        over_limit = place.footprint_m2 > math.min(place.prepared_m2, place.max_buildable_m2)},
      {text = " / ", class = "building_land"}, {text = tostring(math.min(place.prepared_m2, place.max_buildable_m2)), class = "building_land"},
      {text = " m²", class = "building_land"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "storage: ", term = "storage"},
      {text = tostring(place.storage_used_l), class = "storage", over_limit = place.storage_used_l > place.storage_capacity_l},
      {text = " / ", class = "storage"}, {text = tostring(place.storage_capacity_l), class = "storage"}, {text = " L", class = "storage"},
    }, place = place.id},
    {tooltip_parts = {
      {text = "lost to overflow last month: ", term = "overflow"},
      {text = tostring(place.overflow_lost), class = "overflow", over_limit = place.overflow_lost > 0},
    }, place = place.id},
  }
  local price_right = content_width - 82
  local base_right = content_width - 56
  local bar_x = content_width - 48
  local widest_price = 0
  for _, price in ipairs(place.prices) do
    widest_price = math.max(widest_price, font.layout(milli(price.price_milli)).width)
  end
  local name_width = math.max(0, price_right - widest_price - 4 - 16)
  local header = {cells = {
    {text = "good", x = 16, align = "left", colour = theme.colour("term")},
    {text = "price", x = price_right, align = "right", colour = theme.colour("term")},
    {text = "base", x = base_right, align = "right", colour = theme.colour("term")},
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
    local row = {
      -- What the list bar sorts and filters by (CK-296): the name as shown before it is clipped, and the good's tags.
      sort = {name = name, price = price.price_milli, base_price = price.base, buy_sell = value},
      filter = {tag = price.tags},
      cells = {
        {text = clip_column(name, name_width), x = 16, align = "left", colour = name_colour},
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
  self.bar, self.bar_key = nil, nil
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
  -- Also what holds the market's columns beside the scroll bar of its list: they end 48 pixels from the row's right edge (the balance
  -- bar is 46 wide from there), and the bar takes 5 pixels of the content's width (CK-299).
  local columns = (48 + scrollbar.RESERVE // P) * P + 6 * P
  return math.max(windows.min_outer_width(tabs.reserved(TABS), false), columns)
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

local function row_identity(kind, item, index)
  return kind .. ":" .. index .. ":" .. tostring(item.kind) .. ":"
    .. tostring(item.culture or (item.methods[1] ~= nil and item.methods[1].id) or "")
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
      {key = "wage", label = "wage", first = "descending"},
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
      row.sort = {kind = item.kind, positions = item.filled, wage = item.wage}
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
      local id = row_identity("p", pop, index)
      local item = pop_row(pop, id == self.open_pops, place.id, index)
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
    header({{text = "building", x = 16}, {text = "positions", right = 44}, {text = "wage", right = 3}})
    for _, entry in ipairs(shown) do
      local building, index = entry.item, entry.index
      local id = row_identity("b", building, index)
      local item = building_row(building, id == self.open_buildings, place.id, index)
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

local function card_items(key, place, reason, shown)
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
    items[#items + 1] = key == "pops" and pop_card(item, place.id, index) or building_card(item, place.id, index)
  end
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
    ensure_bar(self)
    local strip_top = math.max(0, self.window.content_height - 19 * P)
    local shown = self.bar:view(bar_rows(self.key, place))
    local items = self.list_look == "cards" and card_items(self.key, place, reason, shown)
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
    local lines = place ~= nil and location.lines(self.key, place, width) or {reason}
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
-- place in it, on the tab that is showing.
function location:show(selection, tab)
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
    tooltip.attach(created.title_key, {class = "location", args = {place = selection.id}})
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
  tooltip.attach(self.window.title_key, {class = "location", args = {place = selection.id}})
  self.window:raise()
  if tab ~= nil then
    self.tabs:select(tab)
  end
  render(self, false)
end

-- Tells the panel the newest revision `game_time` carried. A new revision refreshes an open panel, keeping its scroll; the same one
-- (a date alone, a speed, a pause) does nothing.
function location:set_revision(revision)
  price_revision = revision
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
