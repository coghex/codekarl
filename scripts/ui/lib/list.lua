-- A vertical scroll list inside a window's content area (milestone 8, slice 6; docs/ui-design.md §3.2, §3.4, §3.5).
--
-- Everything in and out is in window points on the UI-pixel grid. A row is `(11 + 2) * P` points high: the font's 11 rows and one UI
-- pixel above and below.
--
-- **Two places for the list's parts.**
-- - The viewport hit, `<l>_area`, covers the visible rectangle in the window's *content* group and never scrolls, so the wheel and
--   presses reach the list at every scroll position, over its rows or over empty space.
-- - The rows live in the list's own group, attached to the window, above the content group. Its offset carries the scroll
--   (`origin + (x, y - s)`) and its clip is the visible rectangle cut to the content area, so a scroll is one offset change.
--
-- **The scroll bar** (CK-299, `lib/scrollbar.lua`). While the items are taller than the view the list has a bar at the view's right edge,
-- and its rows are 5 UI pixels narrower than the view: the hover and pressed fills and the row hit regions are `row_w` wide. The list
-- cannot move the cells a caller laid out, so the caller asks `list.row_width(width, rows, count)` for the width of a row of a list of
-- this width and this many rows that holds `count` items, before it lays the cells out. The bar appears or goes only in `list.new` and
-- `list:set_items`, never in a scroll.
--
-- Only the rows that intersect the visible rectangle have glyph quads and, with `on_select`, hit keys; at most `rows + 1`. Two
-- highlight quads (hover and pressed), created first so that the text is above them, are moved to the row that has the look and
-- parked at `ui.HIDDEN` otherwise, since one pointer has at most one hovered row and one pressed row.
local ui = require("lib.ui")
local text = require("lib.text")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local scrollbar = require("lib.scrollbar")

local P = ui.P
local ROW = 13 * P
local MAX_ROWS = 256
local MAX_ITEMS = 65536

local list = {}
list.__index = list

local function finite(value)
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

local function clamp(value, low, high)
  return math.max(low, math.min(value, high))
end

-- Checks `items` (a sequence of strings or column rows), raising an error naming `who`, and returns a copy.
local function checked_items(who, items)
  if type(items) ~= "table" then
    error(who .. ": items must be an array of strings or rows", 3)
  end
  local count = #items
  if count > MAX_ITEMS then
    error(who .. ": at most " .. MAX_ITEMS .. " items", 3)
  end
  local seen = 0
  for _ in pairs(items) do
    seen = seen + 1
  end
  if seen ~= count then
    error(who .. ": items must be an array of strings or rows", 3)
  end
  local copy = {}
  for index = 1, count do
    local item = items[index]
    if type(item) == "string" then
      copy[index] = item
    elseif type(item) == "table" and type(item.cells) == "table" then
      local cells = item.cells
      local cell_count = #cells
      local cell_entries = 0
      for _ in pairs(cells) do cell_entries = cell_entries + 1 end
      if cell_entries ~= cell_count then
        error(who .. ": item " .. index .. " cells must be an array", 3)
      end
      local checked = {cells = {}}
      if item.bar ~= nil then
        local bar = item.bar
        if type(bar) ~= "table" or not finite(bar.x) or bar.x < 0 or bar.x ~= math.floor(bar.x)
          or not finite(bar.width) or bar.width < 3 or bar.width ~= math.floor(bar.width)
          or (bar.value ~= nil and (not finite(bar.value) or bar.value < -1 or bar.value > 1)) then
          error(who .. ": item " .. index .. " bar needs whole x >= 0, whole width >= 3 and a value from -1 to 1 or nil", 3)
        end
        checked.bar = {x = bar.x, width = bar.width, value = bar.value}
      end
      if item.stripe ~= nil and type(item.stripe) ~= "string" then
        error(who .. ": item " .. index .. " stripe must be a colour string", 3)
      end
      checked.stripe = item.stripe
      if item.tooltip ~= nil then
        local source = item.tooltip
        if type(source) == "function" then
          checked.tooltip = source
        elseif type(source) == "table" and type(source.class) == "string" and source.class ~= ""
          and (source.args == nil or type(source.args) == "table") then
          checked.tooltip = {class = source.class, args = source.args}
        else
          error(who .. ": item " .. index .. " tooltip must be a function or {class, args}", 3)
        end
      end
      if item.icon ~= nil then
        if type(item.icon) ~= "table" then
          error(who .. ": item " .. index .. " icon must use exactly one of name and colour or image", 3)
        end
        local has_image = item.icon.image ~= nil
        local has_name = item.icon.name ~= nil
        local has_colour = item.icon.colour ~= nil
        if has_image then
          if has_name or has_colour or type(item.icon.image) ~= "string" or item.icon.image == "" then
            error(who .. ": item " .. index .. " icon must use exactly one of name and colour or image", 3)
          end
          checked.icon = {image = item.icon.image}
        elseif not has_name or not has_colour or type(item.icon.name) ~= "string" or type(item.icon.colour) ~= "string" then
          error(who .. ": item " .. index .. " icon must use exactly one of name and colour or image", 3)
        else
          checked.icon = {name = item.icon.name, colour = item.icon.colour}
        end
      end
      for cell_index = 1, cell_count do
        local cell = cells[cell_index]
        if type(cell) ~= "table" or type(cell.text) ~= "string" or not finite(cell.x)
          or (cell.align ~= "left" and cell.align ~= "right")
          or (cell.colour ~= nil and type(cell.colour) ~= "string")
          or (cell.over_limit ~= nil and type(cell.over_limit) ~= "boolean") then
          error(who .. ": item " .. index .. " cell " .. cell_index .. " needs text, finite x, left/right align, and optional colour", 3)
        end
        checked.cells[cell_index] = {text = cell.text, x = cell.x, align = cell.align,
          colour = cell.colour, over_limit = cell.over_limit}
      end
      copy[index] = checked
    else
      error(who .. ": item " .. index .. " is not a string or row table", 3)
    end
  end
  return copy
end

local function recompute(self)
  self.max_s = math.max(0, (#self.items - self.rows) * ROW)
end

-- The width of a row of a list `width` points wide, `rows` rows high, that holds `count` items.
function list.row_width(width, rows, count)
  if not finite(width) or width <= 0 or not finite(rows) or rows < 1 or not finite(count) or count < 0 then
    error("list.row_width: width, rows and count must be numbers (width > 0, rows >= 1, count >= 0)", 2)
  end
  return scrollbar.row_width(ui.snap_size(width), rows * ROW, count * ROW)
end

-- The rows that intersect the visible rectangle at the current scroll: `first .. last` (empty when last < first).
local function visible_range(self)
  if #self.items == 0 then
    return 1, 0
  end
  local first = math.floor(self.s / ROW) + 1
  local last = math.min(#self.items, math.ceil((self.s + self.view_height) / ROW))
  return first, last
end

local function row_top(index)
  return (index - 1) * ROW
end

-- Puts the two highlight quads on the hovered and the pressed rows, or parks them. A look that is already current makes no call.
local function highlight(self)
  local hover = self.hover_row
  if hover == self.pressed_row then
    hover = nil
  end
  for _, which in ipairs({{"hover_quad", "shown_hover", hover}, {"pressed_quad", "shown_pressed", self.pressed_row}}) do
    local quad, shown, row = self[which[1]], which[2], which[3]
    if self[shown] ~= row then
      if row == nil then
        render.set_position(quad, ui.HIDDEN, ui.HIDDEN)
      else
        render.set_position(quad, 0, row_top(row))
      end
      self[shown] = row
    end
  end
end

local refresh_visible

-- Takes the bar's account of the items and the scroll, and the row width it makes: the fills and the rows' hit regions follow it.
local function refresh_bar(self)
  self.bar:update(self.view_height, #self.items * ROW, self.s)
  local row_w = scrollbar.row_width(self.width, self.view_height, #self.items * ROW)
  if row_w == self.row_w then
    return
  end
  self.row_w = row_w
  render.set_rect(self.hover_quad, ui.HIDDEN, ui.HIDDEN, row_w, ROW)
  render.set_rect(self.pressed_quad, ui.HIDDEN, ui.HIDDEN, row_w, ROW)
  self.shown_hover, self.shown_pressed = nil, nil
  for index, key in pairs(self.row_keys) do
    render.set_hit(key, {{x = 0, y = row_top(index), width = row_w, height = ROW}}, self.rows_group)
  end
  highlight(self)
end

local function release(self, index, key)
  if self.pressed_row == index then
    self.pressed_row = nil
    pointer.pressed(key, false)
    highlight(self)
    if refresh_visible ~= nil then refresh_visible(self) end
  end
end

-- The pointer widget of row `index`, registered under `key`.
local function row_widget(self, index, key)
  if self.on_select == nil then
    return {
      on_event = function(_, event)
        if not self.closed and event.kind == "scroll" then self:scroll(event) end
      end,
      on_hover = function() end,
      on_overflow = function() end,
    }
  end
  local row = {}
  function row.on_event(_, event, eligible)
    if self.closed then
      return
    end
    local kind = event.kind
    if kind == "scroll" then
      self:scroll(event)
    elseif kind == "down" then
      if self.pressed_row == nil then
        self.pressed_row = index
        pointer.pressed(key, true)
        highlight(self)
        if refresh_visible ~= nil then refresh_visible(self) end
      end
    elseif kind == "up" then
      if self.pressed_row == index then
        release(self, index, key)
        if eligible ~= false and event.inside == true and self.on_select ~= nil and self.items[index] ~= nil then
          self.on_select(index)
        end
      end
    elseif kind == "cancel" then
      release(self, index, key)
    end
  end
  function row.on_hover(_, hovered)
    if self.closed then
      return
    end
    if hovered then
      self.hover_row = index
    elseif self.hover_row == index then
      self.hover_row = nil
    end
    highlight(self)
    if refresh_visible ~= nil then refresh_visible(self) end
  end
  function row.functional()
    return self.on_select ~= nil and self.items[index] ~= nil
  end
  function row.on_overflow()
    if self.closed then
      return
    end
    release(self, index, key)
  end
  return row
end

local function destroy_bar(row_view)
  local bar = row_view.bar
  if bar == nil then return end
  render.destroy_quad(bar.track)
  render.destroy_quad(bar.tick)
  if bar.fill ~= nil then render.destroy_quad(bar.fill) end
  row_view.bar = nil
end

local function set_bar(self, row_view, row_index, spec)
  if spec == nil then
    destroy_bar(row_view)
    return
  end
  local left = spec.x * P
  local top = row_top(row_index)
  local center = left + math.floor(spec.width / 2) * P
  local bar = row_view.bar
  if bar == nil then
    bar = {}
    bar.track = render.create_quad{texture = ui.texture("ui/theme/white.png"), x = left, y = top + 5 * P,
      width = spec.width * P, height = 3 * P, group = self.rows_group, tint = theme.colour("track")}
    bar.tick = render.create_quad{texture = ui.texture("ui/theme/white.png"), x = center, y = top + 4 * P,
      width = P, height = 5 * P, group = self.rows_group, tint = theme.colour("dim")}
    row_view.bar = bar
  end

  render.set_rect(bar.track, left, top + 5 * P, spec.width * P, 3 * P)
  render.set_rect(bar.tick, center, top + 4 * P, P, 5 * P)

  local n = 0
  if spec.value ~= nil and spec.value ~= 0 then
    local half = math.floor(spec.width / 2)
    n = math.min(half - 1, math.floor(math.abs(spec.value) * half + 0.5))
  end
  if n == 0 then
    if bar.fill ~= nil then render.destroy_quad(bar.fill); bar.fill = nil end
    return
  end
  local x = spec.value < 0 and center - n * P or center + P
  local colour = theme.colour(spec.value < 0 and "deficit" or "surplus")
  if bar.fill == nil then
    bar.fill = render.create_quad{texture = ui.texture("ui/theme/white.png"), x = x, y = top + 5 * P,
      width = n * P, height = 3 * P, group = self.rows_group, tint = colour}
  else
    render.set_rect(bar.fill, x, top + 5 * P, n * P, 3 * P)
    render.set_tint(bar.fill, colour)
  end
end

local function remove_row(self, index)
  local row_view = self.lines[index]
  if row_view ~= nil then
    for _, line in ipairs(row_view.lines) do text.destroy(line) end
    if row_view.stripe ~= nil then render.destroy_quad(row_view.stripe) end
    if row_view.icon ~= nil then render.destroy_quad(row_view.icon) end
    destroy_bar(row_view)
    self.lines[index] = nil
  end
  local key = self.row_keys[index]
  if key ~= nil then
    -- The hit region goes with the row, at once. The row's widget stays registered until the next dispatch ends, so the events the UI
    -- has not been given yet for the key (routed against the frame the player pointed at) still select the item they meant; a press
    -- and a hover on it end then.
    render.set_hit(key, nil, self.rows_group)
    tooltip.detach(key)
    self.row_keys[index] = nil
    local retired = pointer.widget(key)
    self.retired[key] = true
    pointer.defer(function()
      self.retired[key] = nil
      if pointer.widget(key) == retired then
        pointer.unregister(key)
      end
      if self.row_keys[index] == nil then
        if self.pressed_row == index then
          self.pressed_row = nil
        end
        if self.hover_row == index then
          self.hover_row = nil
        end
        if not self.closed then
          highlight(self)
          if refresh_visible ~= nil then refresh_visible(self) end
        end
      end
    end)
  end
end

local function set_cell(self, row_view, row_index, index, cell)
  local line = row_view.lines[index]
  if line == nil then
    line = text.line(0, 0, row_view.group, cell.colour)
    row_view.lines[index] = line
  end
  local colour = cell.colour or self.colours.text
  if self.numbers or cell.over_limit then
    local highlighted = self.hover_row == row_index or self.pressed_row == row_index
    local number_colour = cell.over_limit and (highlighted and self.colours.highlight_over_limit or theme.colour("over_limit"))
      or highlighted and self.colours.highlight_number or theme.colour("number")
    text.set_segments(line, text.colour_numbers(cell.text, colour, number_colour))
  else
    text.set(line, cell.text)
    text.set_colour(line, colour)
  end
  local x = cell.x * P
  if cell.align == "right" then x = x - line.width * P end
  text.move(line, x, row_view.y + P)
end

local function update_visual(self, index)
  local item = self.items[index]
  local row_view = self.lines[index]
  local is_cells = type(item) == "table"
  local wanted_count = is_cells and #item.cells or 1
  if row_view.is_cells ~= is_cells or #row_view.lines ~= wanted_count then
    for _, line in ipairs(row_view.lines) do text.destroy(line) end
    row_view.lines = {}
    if row_view.stripe ~= nil then render.destroy_quad(row_view.stripe); row_view.stripe = nil end
    if row_view.icon ~= nil then render.destroy_quad(row_view.icon); row_view.icon = nil end
    if not is_cells then destroy_bar(row_view) end
    row_view.icon_source = nil
    row_view.is_cells = is_cells
  end
  row_view.group, row_view.y = self.rows_group, row_top(index)
  if not is_cells then
    set_cell(self, row_view, index, 1, {text = item, x = 1, align = "left", colour = self.colours.text})
    return
  end
  set_bar(self, row_view, index, item.bar)
  for cell_index, cell in ipairs(item.cells) do set_cell(self, row_view, index, cell_index, cell) end
  if item.stripe ~= nil then
    if row_view.stripe == nil then
      row_view.stripe = render.create_quad{texture = ui.texture("ui/theme/white.png"), x = 0, y = row_top(index), width = 2 * P,
        height = ROW, group = self.rows_group, tint = item.stripe}
    else
      render.set_tint(row_view.stripe, item.stripe)
    end
  elseif row_view.stripe ~= nil then
    render.destroy_quad(row_view.stripe)
    row_view.stripe = nil
  end
  if item.icon ~= nil then
    local source = item.icon.image ~= nil and ("image:" .. item.icon.image) or ("theme:" .. item.icon.name)
    if row_view.icon ~= nil and row_view.icon_source ~= source then
      render.destroy_quad(row_view.icon)
      row_view.icon = nil
    end
    if row_view.icon == nil then
      if item.icon.image ~= nil then
        row_view.icon = render.create_quad{texture = ui.texture("ui/" .. item.icon.image .. ".png"), x = 4 * P,
          y = row_top(index) + 2 * P, width = 9 * P, height = 9 * P, group = self.rows_group}
      else
        row_view.icon = render.create_quad{texture = ui.texture("ui/theme/icon_" .. item.icon.name .. ".png"), x = 4 * P,
          y = row_top(index) + P, width = 9 * P, height = 11 * P, group = self.rows_group, tint = item.icon.colour}
      end
      row_view.icon_source = source
    else
      if item.icon.image == nil then render.set_tint(row_view.icon, item.icon.colour) end
    end
  elseif row_view.icon ~= nil then
    render.destroy_quad(row_view.icon)
    row_view.icon = nil
    row_view.icon_source = nil
  end
end

refresh_visible = function(self)
  for index in pairs(self.lines) do update_visual(self, index) end
end

local function add_row(self, index)
  self.lines[index] = {lines = {}, group = self.rows_group, y = row_top(index)}
  update_visual(self, index)
  if self.on_select ~= nil or self.tooltips then
    local key = self.base .. "_row_" .. index
    self.row_keys[index] = key
    render.set_hit(key, {{x = 0, y = row_top(index), width = self.row_w, height = ROW}}, self.rows_group)
    pointer.register(key, row_widget(self, index, key), self.window)
    if self.tooltips then
      tooltip.attach(key, function(x, y)
        if self.closed then return nil end
        local item = self.items[index]
        if type(item) ~= "table" then return nil end
        local source = item.tooltip
        if type(source) == "function" then
          if self.origin_x == nil or self.origin_y == nil then return nil end
          local lx = x - (self.origin_x + self.x)
          local ly = y - (self.origin_y + self.y - self.s + row_top(index))
          local cell_index
          if item.icon ~= nil and lx >= 4 * P and lx < 13 * P then
            cell_index = "icon"
          else
            for cell, spec in ipairs(item.cells) do
              local line = self.lines[index].lines[cell]
              if line ~= nil then
                local right = spec.x * P
                local width = line.width * P
                local inside = spec.align == "right"
                  and lx >= right - width and lx < right
                  or spec.align ~= "right" and lx >= right and lx < right + width
                if inside then cell_index = cell; break end
              end
            end
            if cell_index == nil and item.bar ~= nil then
              local left = item.bar.x * P
              if lx >= left and lx < left + item.bar.width * P then cell_index = "bar" end
            end
          end
          return source(lx, ly, cell_index)
        end
        if type(source) == "table" then return tooltip.content(source.class, source.args) end
        return nil
      end)
    end
  end
end

-- Makes the rows that exist the rows that intersect the visible rectangle, and their text the items'.
local function sync(self)
  local first, last = visible_range(self)
  local stale = {}
  for index in pairs(self.lines) do
    if index < first or index > last then
      stale[#stale + 1] = index
    end
  end
  table.sort(stale)
  for _, index in ipairs(stale) do
    remove_row(self, index)
  end
  for index = first, last do
    if self.lines[index] == nil then
      add_row(self, index)
    else
      update_visual(self, index)
    end
  end
end

-- Moves the rows group and its clip to where the window's content origin and the scroll put them. An empty intersection with the
-- content area hides the group. A value that is already current makes no call.
local function place(self)
  if self.closed or self.origin_x == nil then
    return
  end
  self.bar:place(self.origin_x + self.x + math.max(0, self.width - scrollbar.WIDTH), self.origin_y + self.y, self.content_clip)
  local offset_x = self.origin_x + self.x
  local offset_y = self.origin_y + self.y - self.s
  if offset_x ~= self.shown_offset_x or offset_y ~= self.shown_offset_y then
    render.set_group_offset(self.rows_group, offset_x, offset_y)
    self.shown_offset_x, self.shown_offset_y = offset_x, offset_y
  end
  local clip = self.content_clip
  local left = math.max(self.origin_x + self.x, clip.x)
  local top = math.max(self.origin_y + self.y, clip.y)
  local right = math.min(self.origin_x + self.x + self.width, clip.x + clip.width)
  local bottom = math.min(self.origin_y + self.y + self.view_height, clip.y + clip.height)
  if right <= left or bottom <= top then
    if self.shown_visible ~= false then
      render.set_group_visible(self.rows_group, false)
      self.shown_visible = false
    end
    return
  end
  if left ~= self.shown_left or top ~= self.shown_top or right ~= self.shown_right or bottom ~= self.shown_bottom then
    render.set_group_clip(self.rows_group, {x = left, y = top, width = right - left, height = bottom - top})
    self.shown_left, self.shown_top, self.shown_right, self.shown_bottom = left, top, right, bottom
  end
  if self.shown_visible ~= true then
    render.set_group_visible(self.rows_group, true)
    self.shown_visible = true
  end
end

-- `list.new{window, x, y, width, rows, items, on_select}` (see the file header). Every argument is checked before anything is
-- created.
function list.new(spec)
  if type(spec) ~= "table" then
    error("list.new: expected one table", 2)
  end
  local window = spec.window
  if type(window) ~= "table" or window.attach == nil or window.closed then
    error("list.new: window must be an open window", 2)
  end
  if not finite(spec.x) or not finite(spec.y) then
    error("list.new: x and y must be finite numbers", 2)
  end
  if not finite(spec.width) or spec.width <= 0 then
    error("list.new: width must be a finite number greater than 0", 2)
  end
  local rows = spec.rows
  if not finite(rows) or rows ~= math.floor(rows) or rows < 1 or rows > MAX_ROWS then
    error("list.new: rows must be an integer from 1 to " .. MAX_ROWS, 2)
  end
  local items = checked_items("list.new", spec.items)
  if spec.on_select ~= nil and type(spec.on_select) ~= "function" then
    error("list.new: on_select must be nil or a function", 2)
  end
  if spec.tooltips ~= nil and type(spec.tooltips) ~= "boolean" then
    error("list.new: tooltips must be a boolean", 2)
  end
  if spec.numbers ~= nil and type(spec.numbers) ~= "boolean" then
    error("list.new: numbers must be a boolean", 2)
  end

  local self = setmetatable({}, list)
  self.window = window
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.width = ui.snap_size(spec.width)
  self.rows = math.tointeger(rows)
  self.items = items
  self.on_select = spec.on_select
  self.tooltips = spec.tooltips == true
  self.numbers = spec.numbers == true
  self.base = ui.key("list")
  self.area_key = self.base .. "_area"
  self.view_height = self.rows * ROW
  self.pos = 0
  self.s = 0
  self.lines = {}
  self.row_keys = {}
  self.retired = {} -- keys of removed rows whose widgets stay registered until the next dispatch ends
  self.closed = false
  self.colours = theme.kind(window.kind)
  recompute(self)
  self.row_w = scrollbar.row_width(self.width, self.view_height, #items * ROW)

  self.rows_group = ui.create_group()
  self.bar = scrollbar.new{window = window, owner = {
    scroll = function() return self.s end,
    set_scroll = function(pos) self:set_pos(pos) end,
    wheel = function(event) self:scroll(event) end,
  }}
  self.groups = {self.rows_group, self.bar.group}
  -- The highlights are created before any text, so that the glyphs are drawn over them.
  self.hover_quad = render.create_quad{
    texture = ui.texture("ui/theme/white.png"),
    x = ui.HIDDEN,
    y = ui.HIDDEN,
    width = self.row_w,
    height = ROW,
    group = self.rows_group,
    tint = self.colours.row_hover,
  }
  self.pressed_quad = render.create_quad{
    texture = ui.texture("ui/theme/white.png"),
    x = ui.HIDDEN,
    y = ui.HIDDEN,
    width = self.row_w,
    height = ROW,
    group = self.rows_group,
    tint = self.colours.row_pressed,
  }
  -- The viewport hit does not scroll: it is in the window's content group, below the rows.
  render.set_hit(self.area_key, {{x = self.x, y = self.y, width = self.width, height = self.view_height}}, window.content)
  pointer.register(self.area_key, self, window)
  window:attach(self)
  self.bar:update(self.view_height, #self.items * ROW, self.s)
  sync(self)
  return self
end

-- Called by the window: at attach and after every change of its position or content rectangle. `origin_x`, `origin_y` is the
-- content origin and `content_clip` the content rectangle, both in absolute points.
function list:layout(origin_x, origin_y, content_clip)
  self.origin_x, self.origin_y = origin_x, origin_y
  self.content_clip = content_clip
  place(self)
end

-- Applies a scroll event of the area key or of a row key.
function list:scroll(event)
  if self.closed then
    return
  end
  local amount = event.amount or 0
  if event.unit == "lines" then
    self.pos = self.pos - amount * ROW
  else
    self.pos = self.pos - amount
  end
  self:set_pos(self.pos)
end

-- Sets the scroll to `pos` points (clamped), carrying its fraction; one offset change and a row update when the displayed scroll moves.
function list:set_pos(pos)
  self.pos = clamp(pos, 0, self.max_s)
  local s = ui.snap(self.pos)
  if s ~= self.s then
    self.s = s
    place(self)
    self.bar:update(self.view_height, #self.items * ROW, self.s)
    sync(self)
  end
end

function list:on_event(event)
  if event.kind == "scroll" then
    self:scroll(event)
  end
end

function list:on_hover() end
function list:on_overflow() end

-- Replaces the items: the rows still in view keep their glyph quads where their text is unchanged; the scroll is clamped.
function list:set_items(items)
  if self.closed then
    return
  end
  self.items = checked_items("list:set_items", items)
  recompute(self)
  self.pos = clamp(self.pos, 0, self.max_s)
  local s = ui.snap(self.pos)
  local moved = s ~= self.s
  self.s = s
  if moved then
    place(self)
  end
  refresh_bar(self)
  sync(self)
end

-- Changes the scroll by the least amount that makes row `index` wholly visible.
function list:scroll_to(index)
  if self.closed or math.type(index) ~= "integer" or index < 1 or index > #self.items then
    error("list:scroll_to: no such item", 2)
  end
  local top = row_top(index)
  local bottom = top + ROW
  local pos = self.pos
  if top < pos then
    pos = top
  elseif bottom > pos + self.view_height then
    pos = bottom - self.view_height
  end
  self:set_pos(pos)
end

-- The displayed scroll in points, a multiple of the UI pixel.
function list:scroll_position()
  return self.s
end

function list:destroy()
  if self.closed then
    return
  end
  self.closed = true
  local indices = {}
  for index in pairs(self.row_keys) do
    indices[#indices + 1] = index
  end
  table.sort(indices)
  for _, index in ipairs(indices) do
    local key = self.row_keys[index]
    render.set_hit(key, nil, self.rows_group)
    tooltip.detach(key)
    pointer.unregister(key)
  end
  -- Rows retired by an edit since the last dispatch are still registered: they go with the list.
  local retired = {}
  for key in pairs(self.retired) do
    retired[#retired + 1] = key
  end
  table.sort(retired)
  for _, key in ipairs(retired) do
    tooltip.detach(key)
    pointer.unregister(key)
  end
  self.retired = {}
  self.row_keys = {}
  self.lines = {}
  if not self.window.closed then
    render.set_hit(self.area_key, nil, self.window.content)
  end
  pointer.unregister(self.area_key)
  self.bar:destroy()
  render.destroy_group(self.rows_group)
  self.window:detach(self)
end

list.ROW = ROW
list.MAX_ROWS = MAX_ROWS
return list
