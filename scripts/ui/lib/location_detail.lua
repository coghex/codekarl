-- Detail panels for the item selected in a location's list. Each read uses the two revisions delivered together.
local ui = require("lib.ui")
local windows = require("lib.window")
local cards = require("lib.cards")
local tabs = require("lib.tabs")
local scrollbar = require("lib.scrollbar")
local location = require("lib.location")
local theme = require("lib.theme")
local tooltip = require("lib.tooltip")
local draw = require("lib.draw")
local pointer = require("lib.pointer")
local relations = require("lib.character_relations")
local pop_tab = require("lib.character_pop")
local P = ui.P
local detail = {}
detail.__index = detail

local function section(height, paint)
  local item = {height = height}
  item.draw = function(group, _, y)
    local result = draw.view()
    paint(result, group, y // P, {})
    return result
  end
  return item
end

local function heading(result, group, x, y, width, word)
  local used = draw.text(result, group, x, y, word, theme.kind("place").divider, "small")
  draw.rect(result, group, x + used + 3, y + 4, math.max(1, width - used - 3), 1, theme.kind("place").outline)
end

local GOODS = {}
for _, name in ipairs({"oats", "barley", "peas_beans", "vegetables", "apples", "dairy", "beef", "pork", "mutton", "fish", "seabirds",
  "ale", "mead", "wine", "honey", "herbs", "wool", "beeswax", "hides", "linen", "hemp", "hardwood", "peat", "cattle", "pigs", "sheep", "horses"}) do GOODS[name] = true end
local function icon(result, group, folder, name, x, y)
  if folder == "goods" and not GOODS[name] then name = "fallback/unknown" end
  local quad = render.create_quad{texture = ui.texture("ui/" .. folder .. "/" .. name .. ".png"), x = x * P, y = y * P,
    width = 9 * P, height = (folder == "location" and 11 or 9) * P, group = group}
  result:add({destroy = function() render.destroy_quad(quad) end})
end

local function milli(value)
  local whole, part = value // 1000, value % 1000
  if part == 0 then return tostring(whole) end
  return (string.format("%d.%03d", whole, part):gsub("0+$", ""))
end

local function litres(ml)
  local whole, tenth = ml // 1000, (ml % 1000) // 100
  return tostring(whole) .. (tenth == 0 and "" or "." .. tenth)
end

local function stock_section(stock, capacity, label, blights, width, owner)
  if capacity == 0 and #stock == 0 then return nil end
  local used = 0
  for _, good in ipairs(stock) do used = used + good.charged_ml end
  local denominator = math.max(used, capacity)
  local total = litres(used) .. " / " .. litres(capacity) .. " L"
  local total_split = draw.width(label) + 4 + draw.width(total) > width
  local total_parts = total_split and draw.width(total) > width and 2 or 1
  local top = 32 + (total_split and 11 or 0) + (total_parts == 2 and 11 or 0)
  local stock_rows_top = top
  local rows = {}
  for _, good in ipairs(stock) do
    local marked = blights[good.good]
    local entry = glossary.entry(good.good)
    local base = entry and entry.title or good.good:gsub("_", " ")
    local mark = marked and " blight" or ""
    local name = base .. mark
    local share = capacity == 0 and "—" or (good.charged_ml > 0 and good.charged_ml * 100 < capacity and "<1%" or (good.charged_ml * 100 // capacity) .. "%")
    local suffix = good.outside and "outside" or share
    local figure = tostring(good.amount) .. " · " .. suffix
    if draw.width(name) > width - 12 then
      while utf8.len(base) > 0 and draw.width(base .. "…" .. mark) > width - 12 do base = base:sub(1, utf8.offset(base, -1) - 1) end
      name = base .. "…" .. mark
    end
    local label_width, figure_width = 12 + draw.width(name), draw.width(figure)
    local bar_width = width - label_width - figure_width - 8
    local lines = bar_width >= 12 and 1 or (label_width + figure_width + 4 <= width and 2 or 3)
    if good.outside then lines = label_width + figure_width + 4 <= width and 1 or 2 end
    rows[#rows + 1] = {good = good, marked = marked, name = name, suffix = suffix, figure = figure,
      label_width = label_width, figure_width = figure_width, lines = lines, y = top, bar_width = bar_width}
    top = top + lines * 11
  end
  return relations.item_of({section = section}, top + 1, function(result, group, y, _, hits)
    heading(result, group, 0, y, width, "stock")
    local bar_y = y + 11
    draw.rect(result, group, 0, bar_y, width, 3, theme.colour("track"))
    local cumulative = 0
    for _, good in ipairs(stock) do
      local left = denominator == 0 and 0 or math.floor(width * cumulative / denominator)
      cumulative = cumulative + good.charged_ml
      local right = denominator == 0 and 0 or math.floor(width * cumulative / denominator)
      draw.rect(result, group, left, bar_y, right - left, 3, theme.colour(blights[good.good] and "event_blight" or "number"))
    end
    draw.text(result, group, 0, y + 17, label, theme.colour("term"))
    local total_y = y + 17 + (total_split and 11 or 0)
    local used_colour = theme.colour(used > capacity and "over_limit" or "number")
    if total_parts == 1 then
      draw.text(result, group, width, total_y, {
        {text = litres(used), colour = used_colour}, {text = " / " .. litres(capacity) .. " L", colour = theme.colour("text")},
      }, theme.colour("text"), nil, "right")
    else
      draw.text(result, group, width, total_y, draw.clip(litres(used) .. " /", width, "small"), used_colour, "small", "right")
      draw.text(result, group, width, total_y + 11, draw.clip(litres(capacity) .. " L", width, "small"), theme.colour("text"), "small", "right")
    end
    if owner.kind == "building" then
      hits[#hits + 1] = {x = 0, y = 11, width = width, height = stock_rows_top - 11,
        tip = function() return tooltip.content("store", {place = owner.place, building = owner.index, revision = owner.revision}) end}
    end
    for _, row in ipairs(rows) do
      local good, row_y = row.good, y + row.y
      local colour = theme.colour(row.marked and "event_blight" or "number")
      icon(result, group, "goods", good.good, 0, row_y)
      draw.text(result, group, 12, row_y, row.name, theme.colour("term"))
      local figure_y = row_y + ((row.lines == 3 or (good.outside and row.lines == 2)) and 11 or 0)
      draw.text(result, group, width, figure_y, {{text = tostring(good.amount), colour = colour},
        {text = " · ", colour = theme.colour("dim")},
        {text = row.suffix, colour = theme.colour((good.outside or capacity == 0) and "dim" or "number")}},
        theme.colour("text"), nil, "right")
      if not good.outside then
        local bar_y2 = row.lines == 1 and row_y + 4 or row_y + (row.lines - 1) * 11 + 4
        local bar_x = row.lines == 1 and row.label_width + 4 or 0
        local bar_width = row.lines == 1 and row.bar_width or width
        draw.rect(result, group, bar_x, bar_y2, bar_width, 3, theme.colour("track"))
        draw.rect(result, group, bar_x, bar_y2, denominator == 0 and 0 or math.floor(bar_width * good.charged_ml / denominator), 3, colour)
      end
      hits[#hits + 1] = {x = 0, y = row.y, width = math.min(width, row.label_width), height = 10,
        content = tooltip.explanation(good.good)}
    end
  end)
end

local function building_items(self, building, width)
  local items = {}
  local positions = building.filled .. "/" .. (building.filled + building.open)
  local herd_rows, x, row = {}, 0, 0
  for _, herd in ipairs(building.herds) do
    local count = herd.head .. "/" .. herd.maximum
    local needed = 11 + draw.width(count)
    if x > 0 and x + needed > width then x, row = 0, row + 1 end
    herd_rows[#herd_rows + 1] = {herd = herd, x = x, row = row, count = count, width = needed}
    x = x + needed + 10
  end
  items[#items + 1] = relations.item_of({section = section}, 35 + (#herd_rows > 0 and (row + 1) * 11 or 0), function(result, group, y, _, hits)
    draw.text(result, group, 0, y, "at ", theme.colour("dim"), "small")
    draw.text(result, group, draw.width("at ", "small"), y, self.name, theme.colour("term"), "small")
    draw.text(result, group, 0, y + 11, "wage " .. milli(building.wage), theme.colour("text"), "small")
    draw.text(result, group, width, y + 11, "housing " .. building.housing_occupants, theme.colour("text"), "small", "right")
    draw.text(result, group, 0, y + 22, "positions", theme.colour("term"), "small")
    local bar_x = draw.width("positions", "small") + 4
    local bar_width = math.max(0, width - bar_x - draw.width(positions, "small") - 4)
    draw.rect(result, group, bar_x, y + 25, bar_width, 3, theme.colour("track"))
    draw.rect(result, group, bar_x, y + 25, draw.bar_fill(building.filled, building.filled + building.open, bar_width), 3, theme.colour("text"))
    draw.text(result, group, width, y + 22, positions, theme.colour("number"), "small", "right")
    hits[#hits + 1] = {x = 0, y = 0, width = width, height = 10, content = tooltip.content("location", {place = self.place})}
    hits[#hits + 1] = {x = 0, y = 11, width = width // 2, height = 10,
      tip = function() return tooltip.content("wage", {place = self.place, building = self.index, revision = self.revision}) end}
    hits[#hits + 1] = {x = width // 2, y = 11, width = width - width // 2, height = 10, content = tooltip.explanation("housing")}
    hits[#hits + 1] = {x = 0, y = 22, width = width, height = 10,
      tip = function() return tooltip.content("staff", {place = self.place, building = self.index, revision = self.revision}) end}
    for _, entry in ipairs(herd_rows) do
      local top = y + 34 + entry.row * 11
      icon(result, group, "goods", entry.herd.species, entry.x, top)
      draw.text(result, group, entry.x + 11, top, entry.count, theme.colour("text"))
      hits[#hits + 1] = {x = entry.x, y = 34 + entry.row * 11, width = 9, height = 10,
        content = tooltip.explanation(entry.herd.species)}
      hits[#hits + 1] = {x = entry.x + 11, y = 34 + entry.row * 11, width = entry.width - 11, height = 10,
        tip = function() return tooltip.content("herd", {place = self.place, building = self.index,
          species = entry.herd.species, revision = self.revision}) end}
    end
  end)
  local method_rows, method_height = {}, 11
  for _, method in ipairs(building.methods) do
    local name = method.id:gsub("_", " ")
    local level = "level " .. method.levels
    local split = 11 + draw.width(name) + 4 + draw.width(level) > width
    method_rows[#method_rows + 1] = {method = method, name = name, level = level, y = method_height, split = split}
    method_height = method_height + (split and 22 or 11)
  end
  items[#items + 1] = relations.item_of({section = section}, math.max(12, method_height), function(result, group, y, _, hits)
    heading(result, group, 0, y, width, "methods")
    for _, entry in ipairs(method_rows) do
      local top = y + entry.y
      icon(result, group, "location", "icon_method_" .. entry.method.id, 0, top)
      local room = width - 11 - (entry.split and 0 or draw.width(entry.level) + 4)
      local name = entry.name
      if draw.width(name) > room then
        while utf8.len(name) > 0 and draw.width(name .. "…") > room do name = name:sub(1, utf8.offset(name, -1) - 1) end
        name = name .. "…"
      end
      draw.text(result, group, 11, top, name, theme.colour("term"))
      draw.text(result, group, width, top + (entry.split and 11 or 0), entry.level, theme.colour("text"), nil, "right")
      hits[#hits + 1] = {x = 0, y = entry.y, width = math.min(width, 11 + draw.width(name)), height = 10,
        content = tooltip.explanation(entry.method.id)}
    end
  end)
  local made_rows, mx, my = {}, 0, 11
  for _, good in ipairs(building.made) do
    local needed = 12 + draw.width(tostring(good.amount))
    if mx > 0 and mx + needed > width then mx, my = 0, my + 11 end
    made_rows[#made_rows + 1] = {good = good, x = mx, y = my, width = needed}
    mx = mx + needed + 10
  end
  items[#items + 1] = relations.item_of({section = section}, my + 11, function(result, group, y, _, hits)
    heading(result, group, 0, y, width, "made last month")
    if #made_rows == 0 then draw.text(result, group, 0, y + 11, "nothing yet", theme.colour("dim"), "small") end
    for _, entry in ipairs(made_rows) do
      icon(result, group, "goods", entry.good.good, entry.x, y + entry.y)
      draw.text(result, group, entry.x + 12, y + entry.y, tostring(entry.good.amount), theme.colour("number"))
      hits[#hits + 1] = {x = entry.x, y = entry.y, width = 9, height = 9, content = tooltip.explanation(entry.good.good)}
    end
  end)
  local stock = stock_section(building.stock, building.store_capacity_ml, "storage", self.blights, width, self)
  if stock ~= nil then items[#items + 1] = stock end
  return items
end

function detail.new(spec)
  return setmetatable({view = spec.view, top = spec.top, open_character = spec.open_character, on_go = spec.on_go, open_vehicle = spec.open_vehicle}, detail)
end

function detail:is_open() return self.window ~= nil and not self.window.closed end
function detail:close() if self:is_open() then self.window:close() end end

function detail:refresh(revision, events_revision, world)
  if not self:is_open() then return end
  if world ~= self.world then self:close(); return end
  local ok, place = pcall(game.place, self.place, revision)
  if not ok or place == nil then return end
  local item = (self.kind == "building" and place.buildings or place.pops)[self.index]
  if item == nil or item.kind ~= self.item_kind then self:close(); return end
  local got_blights, blights = pcall(game.blights, self.place, events_revision)
  if not got_blights then return end
  self.revision, self.events_revision, self.blights = revision, events_revision, {}
  for _, good in ipairs(blights) do self.blights[good] = true end
  local width = self.window.content_width // P - scrollbar.RESERVE // P
  local items
  if self.kind == "building" then
    items = building_items(self, item, width)
  else
    local got_members, members = pcall(game.pop_members, self.place, self.index - 1, revision)
    if not got_members then return end
    local kit = {section = section, heading = heading, subject = {}, standalone = true,
      open = function(id) pointer.defer(function() if self:is_open() then self.open_character(id, self.world) end end) end}
    items = pop_tab.sections({pop = item, place = self.place, place_name = self.name, slot = self.index - 1,
      revision = revision, characters = members}, width, kit)
    local stock = stock_section(item.stock, item.room_ml, "room", self.blights, width, self)
    if stock ~= nil then items[#items + 1] = stock end
    if game.pop_trade ~= nil then
      local got_trade, trade = pcall(game.pop_trade, self.place, self.index - 1, tostring(self.world))
      if got_trade and trade then
        if #trade.vehicles > 0 then
          items[#items + 1] = section(11, function(result, group, y) heading(result, group, 0, y, width, 'vehicles') end)
          local vehicle_items = require('lib.expedition_panel').vehicle_items(trade, width, function(number)
            local chosen
            for _, vehicle in ipairs(trade.vehicles) do if vehicle.number == number then chosen = vehicle; break end end
            if chosen and self.open_vehicle then
              local place, index, world = self.place, self.index, self.world
              pointer.defer(function() if self:is_open() and self.world == world then self.open_vehicle(chosen, place, index, world) end end)
            end
          end)
          for _, card in ipairs(vehicle_items) do items[#items + 1] = card end
        end
        if #trade.jobs > 0 then
          items[#items + 1] = section(11 + #trade.jobs * 22, function(result, group, y)
            heading(result, group, 0, y, width, 'jobs')
            for n, job in ipairs(trade.jobs) do
              local destination = job.label:match('^trip to (.+)$')
              local label = destination and ('trip to ' .. require('lib.place_names').name_of(destination)) or job.label:gsub('_', ' ')
              local status = job.hours_left and (job.hours_left .. ' h left') or job.state
              draw.text(result, group, 0, y + 11 + (n - 1) * 22, draw.clip(label, width // 2), theme.colour('term'))
              draw.text(result, group, width, y + 11 + (n - 1) * 22, status, theme.colour('text'), nil, 'right')
              local missing = {}; for _, good in ipairs(job.lacks) do missing[#missing + 1] = good.amount .. ' ' .. good.id:gsub('_', ' ') end
              if #missing > 0 then draw.text(result, group, 0, y + 22 + (n - 1) * 22, draw.clip(table.concat(missing, ', '), width), theme.colour('dim')) end
            end
          end)
        end
      end
    end
  end
  if self.stack ~= nil and not self.stack.closed then
    local scroll = self.stack:scroll_position(); self.stack:set_items(items); self.stack:set_pos(scroll)
  else
    self.stack = cards.new{window = self.window, x = 0, y = 0, width = self.window.content_width,
      height = self.window.content_height, gap = 3, items = items}
  end
end

function detail:open(kind, place, name, index, revision, events_revision, world)
  self:close()
  local top, width, height = location.slot(self.view, ui.snap(self.top()))
  local ok, source = pcall(game.place, place, revision)
  if not ok or source == nil then return end
  local item = (kind == "building" and source.buildings or source.pops)[index]
  if item == nil then return end
  self.kind, self.place, self.name, self.index, self.world = kind, place, name, index, world
  self.item_kind = item.kind
  local created
  created = windows.new{title = kind, kind = kind == "pop" and "social" or "place", outer_width = width,
    outer_height = height, x = 0, y = top, draggable = false, on_close = function()
      if self.window == created then self.window, self.stack = nil, nil end
    end}
  self.window = created
  created:set_title("")
  local label = "    " .. item.kind
  self.tabs = tabs.new{window = created, align = "left", items = {{key = "item", label = label, short = label, tiny = label}},
    selected = "item", on_select = function() end}
  local icon_name = kind == "pop" and "pops" or item.kind
  if icon_name == "farm" or icon_name == "holdings" or icon_name == "demesne" or icon_name == "storehouse" or icon_name == "pops" then
    local path = icon_name == "demesne" and "ui/location/icon_demesne.png" or "ui/theme/icon_" .. icon_name .. ".png"
    local quad = render.create_quad{texture = ui.texture(path), x = 0, y = 0, width = 9 * P, height = 11 * P,
      group = created.chrome, tint = theme.kind(kind == "pop" and "social" or "place").title_text}
    created:add_title_item{place_in_title = function()
      render.set_position(quad, self.tabs.buttons[1].x + 2 * P, 2 * P)
    end}
  end
  created:add_title_button{icon = "location", tooltip = "go to", on_click = function() self.on_go(self.place, self.revision) end}
  self:refresh(revision, events_revision, world)
end

-- A child returns only to the same pop the parent showed, under the current revision.
function detail:return_to_pop(place, index, expected_kind, revision, events_revision, world)
  local ok, source = pcall(game.place, place, revision)
  local pop = ok and source and source.pops[index] or nil
  if not pop or pop.kind ~= expected_kind then return false end
  self:open('pop', place, require('lib.place_names').name_of(place), index, revision, events_revision, world)
  return self:is_open()
end

function detail:relayout()
  if not self:is_open() then return end
  local top, width, height = location.slot(self.view, ui.snap(self.top()))
  if self.stack ~= nil then self.stack:destroy(); self.stack = nil end
  self.window:set_outer_size(width, height); self.window:move_to(0, top)
  self:refresh(self.revision, self.events_revision, self.world)
end

return detail
