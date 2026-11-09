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

local function location_lines(place)
  local lines = {
    "people: " .. (place.workers + place.dependents),
    "  workers: " .. place.workers,
    "  dependents: " .. place.dependents,
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
  self.tabs = nil
  self.id = nil
  self.revision = nil
  self.key = TABS[1].key
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

-- Makes the list show the current tab's rows, keeping its scroll when `keep_scroll`.
local function render(self, keep_scroll)
  if not open(self) then
    return
  end
  local place, reason = fetch(self)
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
  local scroll = (self.list ~= nil and not self.list.closed) and self.list:scroll_position() or 0
  if self.list ~= nil and not self.list.closed then
    self.list:destroy()
  end
  self.list = nil
  self.window:set_outer_size(width, height)
  self.window:move_to(0, top)
  render(self, true)
  if self.list ~= nil then
    self.list:set_pos(scroll)
  end
end

-- Opens the panel for `selection` (a `game_select` table or a places-list entry: `id`, `name`, ...) or, when it is open, shows that
-- place in it, on the tab that is showing.
function location:show(selection)
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

location.TABS = TABS
return location
