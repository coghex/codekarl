-- The fixture-fed event log. Rows are read only when the delivered revision changes.
local ui = require("lib.ui")
local windows = require("lib.window")
local cards = require("lib.cards")
local scrollbar = require("lib.scrollbar")
local listbar = require("lib.listbar")
local resize_corner = require("lib.resize_corner")
local theme = require("lib.theme")
local draw = require("lib.draw")
local time = require("lib.time")
local P = ui.P
local log = {}
log.__index = log

local function wrap(text, width)
  local result, line = {}, ""
  for word in text:gmatch("%S+") do
    local joined = line == "" and word or line .. " " .. word
    if draw.width(joined, "small") <= width then line = joined
    else
      if line ~= "" then result[#result + 1] = line; line = "" end
      while draw.width(word, "small") > width and (utf8.len(word) or 0) > 1 do
        local cut = (utf8.len(word) or #word) - 1
        while cut > 1 and draw.width(word:sub(1, utf8.offset(word, cut + 1) - 1), "small") > width do cut = cut - 1 end
        result[#result + 1] = word:sub(1, utf8.offset(word, cut + 1) - 1)
        word = word:sub(utf8.offset(word, cut + 1))
      end
      line = word
    end
  end
  if line ~= "" or #result == 0 then result[#result + 1] = line end
  return result
end

local function item(row, width, index)
  local stamp = time.date(row)
  local indent = draw.width(stamp, "small") + 4
  local sentence, colour
  if row.kind == "storm" then
    sentence = (row.snow and "snowstorm " or "storm ") .. row.action .. ", " .. row.location .. ", " .. row.knots .. " kn"
    colour = theme.colour(row.action == "merged" and "event_merge" or (row.snow and "event_snow" or "event_storm"))
  else
    sentence = "blight " .. row.action .. ", " .. row.good .. ", " .. row.place
    colour = theme.colour("event_blight")
  end
  local lines = wrap(sentence, math.max(1, width - indent))
  return {height = #lines * 10 + 2, draw = function(group, _, y, row_width)
    local result = draw.view()
    local top = y // P
    if index % 2 == 0 then draw.rect(result, group, 0, top, row_width // P, #lines * 10 + 2, theme.colour("band")) end
    draw.text(result, group, 0, top + 1, {{text = stamp, colour = theme.colour("dim")}}, theme.colour("dim"), "small")
    for i, line in ipairs(lines) do
      draw.text(result, group, indent, top + 1 + (i - 1) * 10, {{text = line, colour = colour}}, colour, "small")
    end
    return result
  end}
end

function log.new(spec)
  return setmetatable({view = spec.view, changed = spec.changed, choices = {storms = true, threshold = 34, blights = false, sort = "default", descending = false}, rows = {}}, log)
end
function log:is_open() return self.window ~= nil and not self.window.closed end
function log:close() if self:is_open() then self.window:close() end end
function log:edge()
  if self.choices.sort == "strength" then return "none" end
  return self.choices.sort == "time" and self.choices.descending and "top" or "bottom"
end
function log:scroll_edge()
  if self:edge() == "bottom" then self.stack:scroll_to_end() else self.stack:set_pos(0) end
end
function log:rebuild(reset)
  if not self:is_open() then return end
  local pos = self.stack and self.stack:scroll_position() or 0
  local edge = self.stack and ((self:edge() == "bottom" and self.stack:at_end()) or (self:edge() == "top" and pos == 0))
  local shown = self.bar:view(self.rows)
  local inner = math.max(1, (self.window.content_width - scrollbar.RESERVE) // P)
  local items = {}
  for i, row in ipairs(shown) do items[#items + 1] = item(row, inner, i) end
  if #items == 0 then items[1] = {height = 13, draw = function(group, _, y)
    local result = draw.view(); draw.text(result, group, 0, y // P + 1, {{text = "nothing yet", colour = theme.colour("dim")}}, theme.colour("dim"), "small"); return result
  end} end
  if self.stack then self.stack:destroy() end
  self.stack = cards.new{window = self.window, x = 0, y = 0, width = self.window.content_width,
    height = math.max(P, self.window.content_height - listbar.HEIGHT), gap = 1, items = items}
  if reset or edge then self:scroll_edge() else self.stack:set_pos(pos) end
  if self.corner then self.window:detach(self.corner); self.window:attach(self.corner) end
  if self.bar.picker then self.window:detach(self.bar.picker); self.window:attach(self.bar.picker) end
end
function log:choice_changed()
  local index = self.bar.sort_index
  self.choices.sort = index == 1 and "time" or (index == 2 and "strength" or "default")
  self.choices.descending = index ~= nil and self.bar.direction == "descending" or false
  self.pending = {storms = self.choices.storms, threshold = self.choices.threshold, blights = self.choices.blights, sort = self.choices.sort, descending = self.choices.descending}
  game.set_log_choices(self.choices.storms, self.choices.threshold, self.choices.blights, self.choices.sort, self.choices.descending)
  self:rebuild(true)
end
function log:fit()
  if not self:is_open() then return end
  local w, h = self.window:size()
  w = math.max(300 * P, math.min(w, self.view.width))
  h = math.max(120 * P, math.min(h, self.view.height))
  self.window:set_outer_size(w, h)
  local x, y = self.window:position()
  if self.view.width < 300 * P or self.view.height < 120 * P then
    x, y = self.view.margin or 8, self.view.margin or 8
  else
    x = math.max(0, math.min(x, self.view.width - w))
    y = math.max(0, math.min(y, self.view.height - h))
  end
  self.window:move_to(x, y)
  self:rebuild(false)
end
function log:open()
  if self:is_open() or not self.revision then return end
  local rows = game.events(self.revision)
  self.rows = rows
  for _, row in ipairs(rows) do row.sort = {time = (((row.year * 13 + row.month) * 32 + row.day) * 24 + row.hour), strength = row.knots} end
  local w = self.width or math.max(300 * P, ui.snap(self.view.width * 5 / 8))
  local h = self.height or math.max(120 * P, ui.snap(self.view.height / 2))
  local created
  created = windows.new{title = "event log", kind = "window", outer_width = w, outer_height = h, x = self.x, y = self.y,
    on_close = function()
      self.x, self.y = created:position(); self.width, self.height = created:size()
      self.window, self.stack, self.bar, self.corner = nil, nil, nil, nil
      if self.changed then self.changed(false) end
    end}
  self.window = created
  self.bar = listbar.new{window = created, event_choices = self.choices,
    sorts = {{key = "time", label = "time", first = "descending"}, {key = "strength", label = "strength", first = "descending"}},
    on_change = function() self:choice_changed() end}
  self.bar.sort_index = self.choices.sort == "time" and 1 or (self.choices.sort == "strength" and 2 or nil)
  self.bar.direction = self.bar.sort_index and (self.choices.descending and "descending" or "ascending") or nil
  self.bar.sort_button:set_selected(self.bar.sort_index ~= nil)
  local log_view = self
  self.corner = resize_corner.new{window = created, view = self.view, prefix = "event_resize", min_width = 300 * P, min_height = 120 * P,
    on_begin = listbar.close_open, on_change = function() log_view:rebuild(false) end}
  self:fit(); self:scroll_edge()
  if self.changed then self.changed(true) end
end
-- The place names show differently now (CK-423): the rows are read again at the revision the log holds, so each blight row names its place
-- as the choice shows it, and the log is laid out again, keeping its scroll.
function log:relabel()
  if not self:is_open() or self.revision == nil then return end
  local rows = game.events(self.revision)
  self.rows = rows
  for _, row in ipairs(rows) do row.sort = {time = (((row.year * 13 + row.month) * 32 + row.day) * 24 + row.hour), strength = row.knots} end
  self:rebuild(false)
end
function log:refresh(snapshot)
  if snapshot.events_revision == nil then return end
  local c = self.choices
  if self.world ~= nil and self.world ~= snapshot.world then self.pending = nil end
  self.world = snapshot.world
  if self.pending then
    local p = self.pending
    if p.storms == snapshot.log_storms and p.threshold == snapshot.log_threshold and p.blights == snapshot.log_blights
      and p.sort == snapshot.log_sort and p.descending == snapshot.log_descending then
      self.pending = nil
    else
      snapshot = {events_revision = snapshot.events_revision, world = snapshot.world, log_storms = c.storms,
        log_threshold = c.threshold, log_blights = c.blights, log_sort = c.sort, log_descending = c.descending}
    end
  end
  local changed = c.storms ~= snapshot.log_storms or c.threshold ~= snapshot.log_threshold or c.blights ~= snapshot.log_blights
    or c.sort ~= snapshot.log_sort or c.descending ~= snapshot.log_descending
  c.storms, c.threshold, c.blights = snapshot.log_storms, snapshot.log_threshold, snapshot.log_blights
  c.sort, c.descending = snapshot.log_sort, snapshot.log_descending
  if self:is_open() and changed then
    listbar.close_open()
    self.bar.sort_index = c.sort == "time" and 1 or (c.sort == "strength" and 2 or nil)
    self.bar.direction = self.bar.sort_index and (c.descending and "descending" or "ascending") or nil
    self.bar.filter_button:set_selected(c.storms or c.blights); self.bar.sort_button:set_selected(self.bar.sort_index ~= nil)
  end
  if self.revision ~= snapshot.events_revision then
    self.revision = snapshot.events_revision
    if self:is_open() then
      local rows = game.events(self.revision)
      self.rows = rows
      for _, row in ipairs(rows) do row.sort = {time = (((row.year * 13 + row.month) * 32 + row.day) * 24 + row.hour), strength = row.knots} end
      self:rebuild(changed)
    end
  elseif changed and self:is_open() then self:rebuild(true) end
end
return log
