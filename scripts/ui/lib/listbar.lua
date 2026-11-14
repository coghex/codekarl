-- The list bar (CK-295, UI polish item 2, slice L2; docs/ui-design.md): two small buttons under a list, the filter and the sort, and the
-- two pickers they open. A bar belongs to one open window and takes the bottom 19 UI pixels of its content area; it does not know the
-- list widget. The caller gives it the list's rows (`bar:view(rows)`) and shows the rows it returns.
--
--   local bar = listbar.new{
--     window = window,                                                     -- an open window; its kind gives the colours
--     sorts = {{key = "name", label = "name", first = "ascending"}, ...},  -- "ascending" or "descending"
--     groups = {{key = "kind", label = "kind"}, ...},                      -- the filter's groups, in order
--     on_change = function() end,                                          -- after a tick, "all" or a sort choice changed the choices
--   }
--   local shown = bar:view(rows)  -- each row has `sort` (key -> string | number | nil) and `filter` (key -> string | {strings} | nil)
--
-- The choices are the bar's alone: nothing of them outlives `destroy`, and nothing is written anywhere.
--
-- **Hit keys.** Every key the bar makes comes from `ui.key` with a short fixed prefix. A value, a label or a group key is data and is never
-- part of a key (the host takes `[a-z0-9_]{1,48}` and a label such as "base price" is not one); the association lives in Lua tables.
local ui = require("lib.ui")
local text = require("lib.text")
local button = require("lib.button")
local panel = require("lib.panel")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local scrollbar = require("lib.scrollbar")

local P = ui.P
local ROW = 13 * P
local HEIGHT = 19 * P
local WHITE = "ui/theme/white.png"

local listbar = {}
listbar.HEIGHT = HEIGHT

local Bar = {}
Bar.__index = Bar
local Picker = {}
Picker.__index = Picker

-- The bar whose picker is open: at most one in the whole UI.
local active = nil
-- The window size of the last `update`: a picker closes when it changes.
local seen_width, seen_height = nil, nil
-- The keys of the pickers destroyed since the last `update`. A press's action can make the picker again (its boxes changed), and the
-- same packet's `down` then names a key of the picker that was destroyed: that is the picker's own key, not an outside press.
local retired = {}

-- ---------------------------------------------------------------------------------------------------------------------------------
-- Ordering
-- ---------------------------------------------------------------------------------------------------------------------------------

local FOLDS = {
  {"\u{E1}", "a"}, {"\u{E9}", "e"}, {"\u{ED}", "i"}, {"\u{F3}", "o"}, {"\u{FA}", "u"},
  {"\u{C1}", "a"}, {"\u{C9}", "e"}, {"\u{CD}", "i"}, {"\u{D3}", "o"}, {"\u{DA}", "u"},
}

-- A text without regard to case, with the acute vowels counted as the plain ones.
local function fold(value)
  value = string.lower(value)
  for _, pair in ipairs(FOLDS) do
    value = string.gsub(value, pair[1], pair[2])
  end
  return value
end

-- -1, 0 or 1. Numbers compare as numbers and texts by `fold`; a number is before a text, so that a mixed column still has one order.
local function compare(a, b)
  local a_number, b_number = type(a) == "number", type(b) == "number"
  if a_number and b_number then
    return a < b and -1 or (a > b and 1 or 0)
  elseif a_number then
    return -1
  elseif b_number then
    return 1
  end
  local left, right = fold(a), fold(b)
  return left < right and -1 or (left > right and 1 or 0)
end

local function values_of(row, group_key)
  local value = row.filter ~= nil and row.filter[group_key] or nil
  if value == nil then return {} end
  if type(value) == "table" then return value end
  return {value}
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- The picker: a cut-3 panel with no title, drawn over everything else of its window
-- ---------------------------------------------------------------------------------------------------------------------------------

local function label_of(value)
  return (string.gsub(value, "_", " "))
end

local function width_of(content)
  return font.layout(content).width
end

-- The checkbox of a box row at local row top `y`: a 7 by 7 outline one pixel thick, in the text colour, from `white.png` quads.
local function checkbox(group, y, colour)
  local quads = {}
  local function add(x, top, width, height)
    quads[#quads + 1] = render.create_quad{texture = ui.texture(WHITE), x = x * P, y = y + top * P, width = width * P,
      height = height * P, group = group, tint = colour}
  end
  add(1, 3, 7, 1)
  add(1, 9, 7, 1)
  add(1, 4, 1, 5)
  add(7, 4, 1, 5)
  return quads
end

local function tick_quad(group, y, colour)
  return render.create_quad{texture = ui.texture(WHITE), x = 3 * P, y = y + 5 * P, width = 3 * P, height = 3 * P, group = group,
    tint = colour}
end

local function release(widget, key)
  if widget.pressed then
    widget.pressed = false
    pointer.pressed(key, false)
  end
end

-- The pointer widget of one row. A press that is released over it does `activate`.
local function row_widget(picker, key, index, activate)
  local widget = {pressed = false}
  function widget:on_event(event, eligible)
    if picker.closed then return end
    local kind = event.kind
    if kind == "scroll" then
      picker:scroll(event)
    elseif kind == "down" then
      if not self.pressed then
        self.pressed = true
        pointer.pressed(key, true)
      end
    elseif kind == "up" then
      if self.pressed then
        release(self, key)
        if eligible ~= false and event.inside == true then activate() end
      end
    elseif kind == "cancel" then
      release(self, key)
    end
  end
  function widget:on_hover(hovered)
    if picker.closed then return end
    if hovered then
      picker.hover = index
    elseif picker.hover == index then
      picker.hover = nil
    end
    picker:show_hover()
  end
  function widget:functional() return true end
  function widget:on_overflow() release(self, key) end
  return widget
end

-- What a picker of `kind` shows: its rows, where they start, how wide its content is, and a signature that is the same for two pickers
-- that would look the same.
local function describe(bar, kind)
  local entries, widest, rows_top, content_width = {}, 0, 0, 0
  if kind == "filter" then
    local multiple = #bar.filters > 1
    for group_index, group in ipairs(bar.filters) do
      if multiple then
        entries[#entries + 1] = {kind = "heading", text = group.label, label = group.label}
        widest = math.max(widest, width_of(group.label))
      end
      for _, box in ipairs(bar.boxes[group_index] or {}) do
        entries[#entries + 1] = {kind = "box", group = group_index, value = box.value, label = box.label}
        widest = math.max(widest, width_of(box.label) + 12)
      end
    end
    widest = math.max(widest, (button.measure("all")) // P)
    rows_top = 16 * P
    content_width = (widest + 4) * P
  else
    entries[1] = {kind = "sort", label = "default"}
    widest = width_of("default")
    for index, choice in ipairs(bar.sorts) do
      entries[index + 1] = {kind = "sort", sort = index, label = choice.label}
      widest = math.max(widest, width_of(choice.label))
    end
    content_width = (widest + 22) * P
  end
  local signature = {}
  for index, entry in ipairs(entries) do
    signature[index] = entry.kind .. "\0" .. tostring(entry.group) .. "\0" .. tostring(entry.value) .. "\0" .. entry.label
  end
  return {entries = entries, rows_top = rows_top, content_width = content_width, signature = table.concat(signature, "\1")}
end

local function new_picker(bar, kind)
  local self = setmetatable({}, Picker)
  local window = bar.window
  self.bar, self.kind, self.window = bar, kind, window
  self.colours = theme.kind(window.kind)
  self.view_width, self.view_height = seen_width, seen_height
  self.owned = {} -- every key of the picker
  self.rows = {} -- row index -> {entry, y, key, line, ...}
  self.pos, self.s = 0, 0
  self.closed = false

  local described = describe(bar, kind)
  local entries = described.entries
  self.entries, self.signature = entries, described.signature
  self.rows_top, self.content_width = described.rows_top, described.content_width
  self.rows_height = #entries * ROW
  self.content_height = self.rows_top + self.rows_height

  local _, _, pw, ph = self:geometry()
  self.group = ui.create_group()
  self.rows_group = ui.create_group()
  -- A picker whose rows overflow its room has the scroll bar (CK-299, `lib/scrollbar.lua`) over its rows' viewport, in the 5 UI pixels its
  -- panel grows by (`geometry`); the bar's keys are the picker's own, so a press on it does not close the picker.
  self.scroll_bar = scrollbar.new{window = window, owner = {
    scroll = function() return self.s end,
    set_scroll = function(pos) self:set_pos(pos) end,
    wheel = function(event) self:scroll(event) end,
  }}
  self.groups = {self.group, self.rows_group, self.scroll_bar.group}
  self.panel = panel.new{x = 0, y = 0, width = pw, height = ph, kind = window.kind, cut = 3, group = self.group}
  -- The panel's own region goes first, so that the "all" button's narrower one is over it. It takes the presses and the wheel over the
  -- blank parts of the panel.
  self.panel_key = ui.key("listbar_panel")
  self.owned[self.panel_key] = true
  render.set_hit(self.panel_key, {{x = 0, y = 0, width = pw, height = ph}}, self.group)
  pointer.register(self.panel_key, {
    on_event = function(_, event)
      if not self.closed and event.kind == "scroll" then self:scroll(event) end
    end,
    on_hover = function() end,
    on_overflow = function() end,
  }, window)

  if kind == "filter" then
    self.all = button.new{x = 4 * P, y = 3 * P, text = "all", kind = window.kind, group = self.group, owner = window,
      on_click = function() bar:clear() end}
    self.owned[self.all.key] = true
  end

  -- The fills are made before the text, so that the text is over them.
  local wide = self.content_width + 6 * P
  self.hover_quad = render.create_quad{texture = ui.texture(WHITE), x = ui.HIDDEN, y = ui.HIDDEN, width = wide, height = ROW,
    group = self.rows_group, tint = self.colours.row_hover}
  if kind == "sort" then
    self.chosen_quad = render.create_quad{texture = ui.texture(WHITE), x = ui.HIDDEN, y = ui.HIDDEN, width = wide, height = ROW,
      group = self.rows_group, tint = self.colours.hot}
  end

  for index, entry in ipairs(entries) do
    local top = (index - 1) * ROW
    local row = {entry = entry, y = top}
    self.rows[index] = row
    if entry.kind == "heading" then
      row.line = text.line(P, top + P, self.rows_group, theme.colour("dim"))
      text.set(row.line, entry.text)
    else
      local key = ui.key(kind == "filter" and "listbar_box" or "listbar_sort")
      row.key = key
      self.owned[key] = true
      render.set_hit(key, {{x = -3 * P, y = top, width = wide, height = ROW}}, self.rows_group)
      local activate
      if entry.kind == "box" then
        row.box = checkbox(self.rows_group, top, self.colours.text)
        activate = function() bar:toggle(entry.group, entry.value) end
        row.line = text.line(12 * P, top + P, self.rows_group, self.colours.text)
      else
        activate = function() bar:choose(entry.sort) end
        row.line = text.line(P, top + P, self.rows_group, self.colours.text)
      end
      text.set(row.line, entry.label)
      pointer.register(key, row_widget(self, key, index, activate), window)
    end
  end
  self:refresh()
  return self
end

-- The rows' viewport of a panel `height` high: what is left under its fixed top part, at least a pixel.
function Picker:viewport_of(height)
  return math.max(height - 6 * P - self.rows_top, P)
end

-- Where the panel is, in content coordinates, and how big: its right edge is the content's right edge, its bottom edge is a pixel above
-- the buttons' top, and its top is never above the content's top: a panel that does not fit is as tall as that leaves. A panel whose
-- rows overflow its viewport is 5 pixels wider, growing to the left, and the scroll bar stands in that room.
function Picker:geometry()
  local window = self.window
  local width = self.content_width + 8 * P
  local limit = window.content_height - 18 * P
  local height = self.content_height + 6 * P
  if height > limit then height = math.max(limit, 9 * P) end
  if self.rows_height > self:viewport_of(height) then width = width + scrollbar.RESERVE end
  return window.content_width - width, math.max(0, limit - height), width, height
end

-- Moves the rows' group to where the panel and the scroll put it.
function Picker:place_rows()
  local px, py = self:geometry()
  render.set_group_offset(self.rows_group, self.origin_x + px + 4 * P, self.origin_y + py + 3 * P + self.rows_top - self.s)
end

-- Puts the panel, its regions and the rows' group where the window's content origin and the scroll put them.
function Picker:apply()
  local px, py, pw, ph = self:geometry()
  local ox, oy = self.origin_x + px, self.origin_y + py
  self.panel:set_rect(0, 0, pw, ph)
  render.set_hit(self.panel_key, {{x = 0, y = 0, width = pw, height = ph}}, self.group)
  render.set_group_offset(self.group, ox, oy)
  local viewport = self:viewport_of(ph)
  self.max_s = math.max(0, self.rows_height - viewport)
  self.pos = math.max(0, math.min(self.pos, self.max_s))
  self.s = ui.snap(self.pos)
  self.viewport = viewport
  self:place_rows()
  render.set_group_clip(self.rows_group, {x = ox + P, y = oy + 3 * P + self.rows_top, width = pw - 2 * P, height = viewport})
  self.scroll_bar:update(viewport, self.rows_height, self.s)
  self.scroll_bar:place(ox + pw - 4 * P, oy + 3 * P + self.rows_top, nil)
end

-- Called by the window: at attach and after every change of its position or content rectangle.
function Picker:layout(origin_x, origin_y)
  self.origin_x, self.origin_y = origin_x, origin_y
  if not self.closed then self:apply() end
end

-- A wheel event over the picker: its rows scroll, as a list's do, and never the list under it.
function Picker:scroll(event)
  local amount = event.amount or 0
  if event.unit == "lines" then
    self:set_pos(self.pos - amount * ROW)
  else
    self:set_pos(self.pos - amount)
  end
end

-- Sets the rows' scroll to `pos` points (clamped); the bar's thumb follows.
function Picker:set_pos(pos)
  if self.closed then return end
  self.pos = math.max(0, math.min(pos, self.max_s or 0))
  local s = ui.snap(self.pos)
  if s ~= self.s then
    self.s = s
    self:place_rows()
    self.scroll_bar:update(self.viewport or P, self.rows_height, s)
  end
end

function Picker:show_hover()
  local row = self.hover
  if row ~= nil and self.chosen_row == row then row = nil end
  if row == self.shown_hover then return end
  if row == nil then
    render.set_position(self.hover_quad, ui.HIDDEN, ui.HIDDEN)
  else
    render.set_position(self.hover_quad, -3 * P, (row - 1) * ROW)
  end
  self.shown_hover = row
end

-- Shows the bar's present choices: the boxes ticked, the "all" button's look, the chosen sort row, its direction mark.
function Picker:refresh()
  local bar = self.bar
  if self.kind == "filter" then
    self.all:set_selected(bar.ticks == 0)
    for _, row in ipairs(self.rows) do
      local entry = row.entry
      if entry.kind == "box" then
        local ticked = bar.ticked[bar.filters[entry.group].key][entry.value] == true
        if ticked and row.tick == nil then
          row.tick = tick_quad(self.rows_group, row.y, self.colours.text)
        elseif not ticked and row.tick ~= nil then
          render.destroy_quad(row.tick)
          row.tick = nil
        end
      end
    end
    return
  end
  local chosen = bar.sort_index == nil and 1 or bar.sort_index + 1
  self.chosen_row = chosen
  render.set_position(self.chosen_quad, -3 * P, (chosen - 1) * ROW)
  for index, row in ipairs(self.rows) do
    local ink = index == chosen and self.colours.pressed_text or self.colours.text
    text.set_colour(row.line, ink)
    if row.mark ~= nil then
      render.destroy_quad(row.mark)
      row.mark = nil
    end
    if index == chosen and bar.sort_index ~= nil then
      local name = bar.direction == "ascending" and "sort_up" or "sort_down"
      row.mark = render.create_quad{texture = ui.texture("ui/theme/" .. name .. ".png"), x = self.content_width - 6 * P,
        y = row.y + 4 * P, width = 5 * P, height = 5 * P, group = self.rows_group, tint = ink}
    end
  end
  self:show_hover()
end

function Picker:owns(key)
  return self.owned[key] == true or self.scroll_bar:owns(key)
end

function Picker:destroy()
  if self.closed then return end
  self.closed = true
  for key in pairs(self.owned) do retired[key] = true end
  if self.scroll_bar.track_key ~= nil then
    retired[self.scroll_bar.track_key], retired[self.scroll_bar.thumb_key] = true, true
  end
  render.set_hit(self.panel_key, nil, self.group)
  pointer.unregister(self.panel_key)
  for _, row in ipairs(self.rows) do
    if row.key ~= nil then
      render.set_hit(row.key, nil, self.rows_group)
      pointer.unregister(row.key)
    end
  end
  if self.all ~= nil then self.all:destroy() end
  self.scroll_bar:destroy()
  self.panel:destroy()
  if not self.window.closed then self.window:detach(self) end
  render.destroy_group(self.rows_group)
  render.destroy_group(self.group)
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- The bar
-- ---------------------------------------------------------------------------------------------------------------------------------

local function close_picker(bar)
  local picker = bar.picker
  if picker == nil then return end
  bar.picker = nil
  if active == bar then active = nil end
  picker:destroy()
end

local function open_picker(bar, kind)
  if bar.closed or bar.window.closed then return end
  if active ~= nil then close_picker(active) end
  local picker = new_picker(bar, kind)
  bar.picker = picker
  active = bar
  bar.window:attach(picker)
end

local function toggle_picker(bar, kind)
  if bar.picker ~= nil and bar.picker.kind == kind then
    close_picker(bar)
  else
    open_picker(bar, kind)
  end
end

local function place_count(self)
  local button_x, button_y = self.filter_button.x, self.filter_button.y
  text.move(self.count, button_x - 5 * P - self.count.width * P, button_y + 2 * P)
end

local function show_count(self)
  if self.ticks > 0 then
    text.set_segments(self.count, {
      {text = tostring(self.shown), colour = theme.colour("number")},
      {text = " of ", colour = theme.colour("dim")},
      {text = tostring(self.total), colour = theme.colour("number")},
    })
  else
    text.set(self.count, "")
  end
  place_count(self)
end

-- `listbar.new{window, sorts, groups, on_change}` (see the file header).
function listbar.new(spec)
  if type(spec) ~= "table" then error("listbar.new: expected one table", 2) end
  local window = spec.window
  if type(window) ~= "table" or window.attach == nil or window.closed then
    error("listbar.new: window must be an open window", 2)
  end
  local sorts, groups = spec.sorts or {}, spec.groups or {}
  for index, choice in ipairs(sorts) do
    if type(choice.key) ~= "string" or type(choice.label) ~= "string"
      or (choice.first ~= "ascending" and choice.first ~= "descending") then
      error("listbar.new: sort " .. index .. " needs a key, a label and `first` of ascending or descending", 2)
    end
  end
  for index, group in ipairs(groups) do
    if type(group.key) ~= "string" or type(group.label) ~= "string" then
      error("listbar.new: group " .. index .. " needs a key and a label", 2)
    end
  end
  if spec.on_change ~= nil and type(spec.on_change) ~= "function" then
    error("listbar.new: on_change must be nil or a function", 2)
  end
  local self = setmetatable({}, Bar)
  self.window = window
  self.sorts, self.filters = sorts, groups
  self.on_change = spec.on_change
  self.ticked = {} -- group key -> value -> true
  self.group_ticks = {} -- group key -> how many boxes of it are ticked
  self.boxes = {} -- group index -> {{value, label}, ...} in label order
  for index, group in ipairs(groups) do
    self.ticked[group.key] = {}
    self.group_ticks[group.key] = 0
    self.boxes[index] = {}
  end
  self.ticks = 0
  self.sort_index = nil -- the chosen sort; nil is the default order
  self.direction = nil
  self.shown, self.total = 0, 0
  self.groups = {} -- the window's attached-item protocol: the bar draws into the window's content group and has none of its own
  self.closed = false
  self.filter_button = button.new{x = 0, y = 0, icons = {"filter"}, icon = "filter", kind = window.kind, group = window.content,
    owner = window, tooltip = "filter", on_click = function() toggle_picker(self, "filter") end}
  self.sort_button = button.new{x = 0, y = 0, icons = {"sort"}, icon = "sort", kind = window.kind, group = window.content,
    owner = window, tooltip = "sort", on_click = function() toggle_picker(self, "sort") end}
  self.count = text.line(0, 0, window.content)
  window:attach(self)
  return self
end

function Bar:layout()
  if self.closed then return end
  local width, height = self.window.content_width, self.window.content_height
  local y = height - 17 * P
  local sort_width = self.sort_button:size()
  self.sort_button:move(width - sort_width, y)
  self.filter_button:move(width - sort_width - 16 * P, y)
  place_count(self)
end

-- The rows that pass the filter, in the chosen order, as a new array of the caller's own tables.
function Bar:view(rows)
  -- The boxes: the values the rows have, and any value still ticked that no row has now.
  local seen = {}
  for index, group in ipairs(self.filters) do
    local values = {}
    for _, row in ipairs(rows) do
      for _, value in ipairs(values_of(row, group.key)) do values[value] = true end
    end
    for value in pairs(self.ticked[group.key]) do values[value] = true end
    local list = {}
    for value in pairs(values) do list[#list + 1] = {value = value, label = label_of(value)} end
    table.sort(list, function(a, b)
      local c = compare(a.label, b.label)
      if c ~= 0 then return c < 0 end
      return a.value < b.value
    end)
    seen[index] = list
  end
  self.boxes = seen

  local passed = {}
  for _, row in ipairs(rows) do
    local ok = true
    for _, group in ipairs(self.filters) do
      if ok and self.group_ticks[group.key] > 0 then
        local match = false
        for _, value in ipairs(values_of(row, group.key)) do
          if self.ticked[group.key][value] then match = true; break end
        end
        ok = match
      end
    end
    if ok then passed[#passed + 1] = row end
  end

  if self.sort_index ~= nil then
    local key, descending = self.sorts[self.sort_index].key, self.direction == "descending"
    local decorated = {}
    for index, row in ipairs(passed) do
      local value = row.sort ~= nil and row.sort[key] or nil
      decorated[index] = {row = row, index = index, value = value, missing = value == nil}
    end
    table.sort(decorated, function(a, b)
      if a.missing ~= b.missing then return b.missing end
      if a.missing then return a.index < b.index end
      local c = compare(a.value, b.value)
      if c ~= 0 then
        if descending then return c > 0 end
        return c < 0
      end
      return a.index < b.index
    end)
    passed = {}
    for index, entry in ipairs(decorated) do passed[index] = entry.row end
  end

  self.shown, self.total = #passed, #rows
  show_count(self)
  local picker = self.picker
  if picker ~= nil then
    if picker.kind == "filter" and describe(self, "filter").signature ~= picker.signature then
      -- The boxes are not the boxes the picker was made with: it is made again, at the same scroll and for the same window size.
      local width, height, scroll = picker.view_width, picker.view_height, picker.pos
      close_picker(self)
      open_picker(self, "filter")
      self.picker.view_width, self.picker.view_height = width, height
      self.picker.pos = scroll
      self.picker:apply()
    end
    self.picker:refresh()
  end
  return passed
end

local function changed(bar)
  bar.filter_button:set_selected(bar.ticks > 0)
  bar.sort_button:set_selected(bar.sort_index ~= nil)
  if bar.picker ~= nil then bar.picker:refresh() end
  tooltip.close_all()
  if bar.on_change ~= nil then bar.on_change() end
end

-- Ticks or unticks the box of `value` in group number `group_index`.
function Bar:toggle(group_index, value)
  local key = self.filters[group_index].key
  if self.ticked[key][value] then
    self.ticked[key][value] = nil
    self.group_ticks[key] = self.group_ticks[key] - 1
    self.ticks = self.ticks - 1
  else
    self.ticked[key][value] = true
    self.group_ticks[key] = self.group_ticks[key] + 1
    self.ticks = self.ticks + 1
  end
  changed(self)
end

-- Unticks every box; nothing changes, and `on_change` is not called, when none is ticked.
function Bar:clear()
  if self.ticks == 0 then return end
  for _, group in ipairs(self.filters) do
    self.ticked[group.key] = {}
    self.group_ticks[group.key] = 0
  end
  self.ticks = 0
  changed(self)
end

-- Chooses sort number `index` (nil is the default order): its first direction, or the other one when it already is the chosen sort.
function Bar:choose(index)
  if index == nil then
    if self.sort_index == nil then return end
    self.sort_index, self.direction = nil, nil
  elseif self.sort_index == index then
    self.direction = self.direction == "ascending" and "descending" or "ascending"
  else
    self.sort_index, self.direction = index, self.sorts[index].first
  end
  changed(self)
end

function Bar:destroy()
  if self.closed then return end
  close_picker(self)
  self.closed = true
  self.filter_button:destroy()
  self.sort_button:destroy()
  text.destroy(self.count)
  if not self.window.closed then self.window:detach(self) end
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- What closes a picker
-- ---------------------------------------------------------------------------------------------------------------------------------

-- Closes the open picker. True when there was one.
function listbar.close_open()
  if active == nil then return false end
  close_picker(active)
  return true
end

-- Called once a frame, after the pointer dispatcher: closes the open picker for a press on the map, a press on any key that is not
-- the picker's own nor the button that opened it (the press has already done what it does), and a change of the window's size. The
-- events of an overflow packet close nothing.
function listbar.update(update)
  local width, height = update.width, update.height
  local bar = active
  if bar ~= nil then
    local picker = bar.picker
    local opener = (picker.kind == "filter" and bar.filter_button or bar.sort_button).key
    local resized = (picker.view_width ~= nil and width ~= nil and width ~= picker.view_width)
      or (picker.view_height ~= nil and height ~= nil and height ~= picker.view_height)
    local pointer_state = update.pointer
    local closes = resized
    if not closes and pointer_state ~= nil and not pointer_state.overflow then
      for _, event in ipairs(pointer_state.events or {}) do
        if event.kind == "map_down" then
          closes = true
        elseif event.kind == "down" and event.key and event.key ~= opener and not picker:owns(event.key)
          and not retired[event.key] then
          closes = true
        end
      end
    end
    if closes then close_picker(bar) end
  end
  seen_width, seen_height = width, height
  retired = {}
end

return listbar
