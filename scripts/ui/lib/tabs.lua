-- Tabs in a window's title bar (milestone 9, slice 3; docs/economy-design.md §3.11): one title-style button for each tab, right-aligned
-- in the room the title leaves before the close button, the selected one drawn pressed. A tab has a label, a short label and a tiny
-- one; the tabs use the longest set that fits the room together, and the tiny set (drawn over the title's end) when none does.
local ui = require("lib.ui")
local button = require("lib.button")

local P = ui.P
local GAP = P
local TITLE_X = 8 * P -- where a window's title starts (window.lua: the cut of 7 and a pixel)

local tabs = {}
tabs.__index = tabs

-- Creates the tabs of `spec.window` (an open window):
--   items     a sequence of `{key = "location", label = "location", short = "loc", tiny = "l"}` or `{key, icon}`;
--   selected  the key of the tab showing first (the first item's by default);
--   on_select called as `on_select(key)` when a tab other than the selected one is clicked.
function tabs.new(spec)
  if type(spec) ~= "table" then
    error("tabs.new: expected one table", 2)
  end
  local window = spec.window
  if type(window) ~= "table" or window.add_title_item == nil or window.closed then
    error("tabs.new: window must be an open window", 2)
  end
  if type(spec.items) ~= "table" or #spec.items == 0 then
    error("tabs.new: items must be a non-empty array", 2)
  end
  local self = setmetatable({}, tabs)
  self.window = window
  self.on_select = spec.on_select
  self.items = {}
  self.buttons = {}
  self.tier = 1
  local seen = {}
  for index, item in ipairs(spec.items) do
    if type(item.key) ~= "string" or (item.icon == nil and type(item.label) ~= "string")
      or (item.icon ~= nil and type(item.icon) ~= "string") then
      error("tabs.new: item " .. index .. " needs a key and a label or icon", 2)
    end
    if seen[item.key] then
      error("tabs.new: key `" .. item.key .. "` is used twice", 2)
    end
    seen[item.key] = true
    local short = item.short or item.label
    self.items[index] = {
      key = item.key,
      icon = item.icon,
      labels = item.label ~= nil and {item.label, short, item.tiny or short} or nil,
    }
  end
  self.selected = spec.selected or self.items[1].key
  if not seen[self.selected] then
    error("tabs.new: selected must be the key of an item", 2)
  end
  for index, item in ipairs(self.items) do
    local button_spec = {
      x = 0, -- placed by `place_in_title` as soon as the tabs are added
      y = 0,
      kind = window.kind,
      style = "title",
      group = window.chrome,
      owner = window,
      on_click = function() self:select(item.key, true) end,
    }
    if item.icon ~= nil then
      button_spec.icons, button_spec.icon = {item.icon}, item.icon
    else
      button_spec.text = item.labels[1]
    end
    self.buttons[index] = button.new(button_spec)
    self.buttons[index]:set_selected(item.key == self.selected)
  end
  window:add_title_item(self)
  return self
end

-- Whether the tabs, with the labels of `tier`, are narrower than `room` points.
local function fits(self, room, tier)
  local total = -GAP
  for _, item in ipairs(self.items) do
    local width = item.icon ~= nil and button.measure_icon(item.icon) or button.measure(item.labels[tier])
    total = total + width + GAP
  end
  return total <= room
end

-- Called by the window: lays the buttons out from the right edge of the room, with the longest labels that fit.
function tabs:place_in_title(window)
  local left, right = window:title_bounds()
  local tier = 3
  for candidate = 1, 2 do
    if fits(self, right - left, candidate) then
      tier = candidate
      break
    end
  end
  if tier ~= self.tier then
    self.tier = tier
    for index, item in ipairs(self.items) do
      if item.labels ~= nil then self.buttons[index]:set_text(item.labels[tier]) end
    end
  end
  -- The buttons end at the right edge of the room and never start before the title bar's own start: a window narrower than
  -- `window.min_outer_width` has no room for them, and they then run on from that start (over the title, past the close button's side).
  local total = -GAP
  for index = 1, #self.items do
    total = total + (self.buttons[index]:size()) + GAP
  end
  local x = math.max(right - total, TITLE_X)
  for index = 1, #self.items do
    local width = self.buttons[index]:size()
    self.buttons[index]:move(x, 0)
    x = x + width + GAP
  end
end

-- The room, in points, a title bar must keep for tabs of `items` (as `tabs.new` takes them): the buttons with their tiny labels and
-- the gaps, and a gap before them.
function tabs.reserved(items)
  local total = GAP
  for _, item in ipairs(items) do
    local width = item.icon ~= nil and button.measure_icon(item.icon) or button.measure(item.tiny or item.short or item.label)
    total = total + width + GAP
  end
  return total
end

-- The same, for these tabs.
function tabs:reserved_width()
  local total = GAP
  for _, item in ipairs(self.items) do
    local width = item.icon ~= nil and button.measure_icon(item.icon) or button.measure(item.labels[3])
    total = total + width + GAP
  end
  return total
end

-- The key of the tab showing.
function tabs:current()
  return self.selected
end

-- Shows the tab `key`; `notify` also calls `on_select` when it was another tab (a click does, a program's choice does not).
function tabs:select(key, notify)
  local found = false
  for index, item in ipairs(self.items) do
    if item.key == key then
      found = true
      self.buttons[index]:set_selected(true)
    else
      self.buttons[index]:set_selected(false)
    end
  end
  if not found then
    error("tabs:select: `" .. tostring(key) .. "` is not a tab", 2)
  end
  local changed = self.selected ~= key
  self.selected = key
  if changed and notify and self.on_select ~= nil then
    self.on_select(key)
  end
end

return tabs
