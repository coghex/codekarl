-- A clipped, variable-height stack for the place panel's pop and building cards/rows (M9 s12e).
-- Items use UI-pixel heights and draw(group, x, y, width) in window points. `draw` returns a view with `destroy()`.
local ui = require("lib.ui")
local pointer = require("lib.pointer")

local P = ui.P
local MAX_ITEMS = 65536
local cards = {}
cards.__index = cards

local function finite(value)
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

local function checked_items(items)
  if type(items) ~= "table" then error("cards: items must be an array", 3) end
  local count, entries = #items, 0
  if count > MAX_ITEMS then error("cards: too many items", 3) end
  for _ in pairs(items) do entries = entries + 1 end
  if entries ~= count then error("cards: items must be a dense array", 3) end
  local copy = {}
  for index = 1, count do
    local item = items[index]
    if type(item) ~= "table" or not finite(item.height) or item.height < 1 or item.height ~= math.floor(item.height)
      or type(item.draw) ~= "function" or (item.on_click ~= nil and type(item.on_click) ~= "function") then
      error("cards: item " .. index .. " needs a positive integer height, draw, and optional on_click", 3)
    end
    copy[index] = item
  end
  return copy
end

local function gap_points(self)
  return self.gap * P
end

local function top_of(self, index)
  return self.tops[index]
end

local function measure(self)
  local total, tops = 0, {}
  for index, item in ipairs(self.items) do
    tops[index] = total
    total = total + item.height * P
    if index < #self.items then total = total + gap_points(self) end
  end
  self.tops = tops
  self.content_height = total
end

local function max_scroll(self)
  return math.max(0, self.content_height - self.height)
end

local function destroy_views(self)
  for index, view in pairs(self.views) do
    if view.destroy ~= nil then view:destroy() end
    self.views[index] = nil
  end
end

local function clear_hits(self)
  local keys = {}
  for key in pairs(self.item_keys) do keys[#keys + 1] = key end
  table.sort(keys)
  for _, key in ipairs(keys) do
    render.set_hit(key, nil, self.group)
    local retired = pointer.widget(key)
    pointer.defer(function()
      if pointer.widget(key) == retired then pointer.unregister(key) end
    end)
    self.item_keys[key] = nil
  end
end

local function visible(self)
  local first, last = nil, nil
  local bottom = self.scroll_offset + self.height
  for index, item in ipairs(self.items) do
    local top = top_of(self, index)
    local end_y = top + item.height * P
    if end_y > self.scroll_offset and top < bottom then
      first = first or index
      last = index
    end
  end
  return first, last
end

local function item_widget(self, index, key)
  local widget = {pressed = false}
  function widget:on_event(event, eligible)
    if self.cards.closed then return end
    if event.kind == "scroll" then
      self.cards:scroll(event)
    elseif event.kind == "down" then
      self.pressed = true
      pointer.pressed(key, true)
    elseif event.kind == "up" then
      local was_pressed = self.pressed
      self.pressed = false
      pointer.pressed(key, false)
      local item = self.cards.items[index]
      if was_pressed and eligible ~= false and event.inside == true and item ~= nil and item.on_click ~= nil then
        item.on_click()
      end
    elseif event.kind == "cancel" then
      self.pressed = false
      pointer.pressed(key, false)
    end
  end
  function widget:on_overflow()
    if self.pressed then
      self.pressed = false
      pointer.pressed(key, false)
    end
  end
  function widget:on_hover() end
  widget.cards = self
  return widget
end

local function sync(self, redraw)
  if self.closed then return end
  if redraw then destroy_views(self) end
  self.generation = self.generation + 1
  clear_hits(self)
  local first, last = visible(self)
  local wanted = {}
  if first ~= nil then
    for index = first, last do
      wanted[index] = true
      if self.views[index] == nil then
        local item = self.items[index]
        local top = top_of(self, index)
        local view = item.draw(self.group, 0, top, self.width)
        if view ~= nil and type(view) ~= "table" then error("cards: draw must return a view or nil", 2) end
        self.views[index] = view or {}
      end
      local key = self.base .. "_item_" .. self.generation .. "_" .. index
      self.item_keys[key] = true
      render.set_hit(key, {{x = 0, y = top_of(self, index), width = self.width, height = self.items[index].height * P}}, self.group)
      pointer.register(key, item_widget(self, index, key), self.window)
    end
  end
  local stale = {}
  for index in pairs(self.views) do if not wanted[index] then stale[#stale + 1] = index end end
  table.sort(stale)
  for _, index in ipairs(stale) do
    local view = self.views[index]
    if view ~= nil and view.destroy ~= nil then view:destroy() end
    self.views[index] = nil
  end
end

local function place(self)
  if self.closed or self.origin_x == nil then return end
  local x, y = self.origin_x + self.x, self.origin_y + self.y - self.scroll_offset
  if x ~= self.shown_x or y ~= self.shown_y then
    render.set_group_offset(self.group, x, y)
    self.shown_x, self.shown_y = x, y
  end
  local left = math.max(self.origin_x + self.x, self.content_clip.x)
  local top = math.max(self.origin_y + self.y, self.content_clip.y)
  local right = math.min(self.origin_x + self.x + self.width, self.content_clip.x + self.content_clip.width)
  local bottom = math.min(self.origin_y + self.y + self.height, self.content_clip.y + self.content_clip.height)
  if right <= left or bottom <= top then
    if self.shown_visible ~= false then render.set_group_visible(self.group, false); self.shown_visible = false end
    return
  end
  if left ~= self.clip_left or top ~= self.clip_top or right ~= self.clip_right or bottom ~= self.clip_bottom then
    render.set_group_clip(self.group, {x = left, y = top, width = right - left, height = bottom - top})
    self.clip_left, self.clip_top, self.clip_right, self.clip_bottom = left, top, right, bottom
  end
  if self.shown_visible ~= true then render.set_group_visible(self.group, true); self.shown_visible = true end
end

function cards.new(spec)
  if type(spec) ~= "table" then error("cards.new: expected one table", 2) end
  local window = spec.window
  if type(window) ~= "table" or window.attach == nil or window.closed then error("cards.new: window must be open", 2) end
  for _, key in ipairs({"x", "y", "width", "height"}) do
    if not finite(spec[key]) or ((key == "width" or key == "height") and spec[key] < 0) then
      error("cards.new: " .. key .. " must be a finite non-negative number", 2)
    end
  end
  if spec.width <= 0 then error("cards.new: width must be positive", 2) end
  if spec.gap ~= 1 and spec.gap ~= 3 then error("cards.new: gap must be 1 or 3 UI pixels", 2) end
  local self = setmetatable({
    window = window, x = ui.snap(spec.x), y = ui.snap(spec.y), width = ui.snap_size(spec.width), height = ui.snap_size(spec.height),
    gap = spec.gap, items = checked_items(spec.items), scroll_offset = 0, views = {}, item_keys = {}, generation = 0, closed = false,
    base = ui.key("cards"),
  }, cards)
  measure(self)
  self.area_key = self.base .. "_area"
  self.group = render.create_group()
  self.groups = {self.group}
  self.area_widget = {}
  function self.area_widget:on_event(event)
    if event.kind == "scroll" then self.cards:scroll(event) end
  end
  function self.area_widget:on_hover() end
  self.area_widget.cards = self
  render.set_hit(self.area_key, {{x = self.x, y = self.y, width = self.width, height = self.height}}, window.content)
  pointer.register(self.area_key, self.area_widget, window)
  window:attach(self)
  sync(self, false)
  return self
end

function cards:layout(origin_x, origin_y, content_clip)
  self.origin_x, self.origin_y, self.content_clip = origin_x, origin_y, content_clip
  place(self)
end

function cards:scroll(event)
  if self.closed then return end
  local amount = event.amount or 0
  local step = event.unit == "lines" and 13 * P or 1
  self:set_pos(self.scroll_offset - amount * step)
end

function cards:set_pos(position)
  if not finite(position) then error("cards:set_pos: position must be finite", 2) end
  local next_scroll = ui.snap(math.max(0, math.min(position, max_scroll(self))))
  if next_scroll ~= self.scroll_offset then
    self.scroll_offset = next_scroll
    place(self)
    sync(self, false)
  end
end

function cards:scroll_position()
  return self.scroll_offset
end

function cards:set_items(items, gap)
  if self.closed then return end
  if gap ~= nil then
    if gap ~= 1 and gap ~= 3 then error("cards:set_items: gap must be 1 or 3", 2) end
    self.gap = gap
  end
  self.items = checked_items(items)
  measure(self)
  self.scroll_offset = ui.snap(math.max(0, math.min(self.scroll_offset, max_scroll(self))))
  place(self)
  sync(self, true)
end

function cards:destroy()
  if self.closed then return end
  self.closed = true
  clear_hits(self)
  destroy_views(self)
  render.set_hit(self.area_key, nil, self.window.content)
  pointer.unregister(self.area_key)
  render.destroy_group(self.group)
  self.window:detach(self)
end

return cards
