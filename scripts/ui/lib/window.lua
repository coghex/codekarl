-- Draggable, clipped UI windows. Geometry and the public API use window points;
-- the renderer applies each group's offset to the window-local contents.
local ui = require("lib.ui")
local frame = require("lib.frame")
local label = require("lib.label")
local button = require("lib.button")
local theme = require("lib.theme")
local pointer = require("lib.pointer")

local P = ui.P
local PAD = 3 * P
local BOTTOM = 5 * P -- the bottom padding: the content rectangle lies wholly inside the bottom-right cut of 7
local CUT = 7
local TITLE_X = (CUT + 1) * P -- the title starts inside the cut title bar
local windows = {}
local viewport = {width = 800, height = 600}
local counter = 0

local window = {}
local Window = {}
Window.__index = Window

local function floor_grid(value)
  return math.floor(value / P) * P
end

local function clamp(value, low, high)
  return math.max(low, math.min(value, high))
end

-- The groups of a window in drawing order: chrome, content, then each attached item's groups in attach order.
local function stack(self)
  local groups = {self.chrome, self.content}
  for _, item in ipairs(self.attached) do
    for _, group in ipairs(item.groups) do
      groups[#groups + 1] = group
    end
  end
  return groups
end

-- Lays the attached items out for the window's current position and content rectangle, in the callback that changed them.
local function layout_items(self, only)
  local origin_x, origin_y = self.x + PAD, self.y + self.title_height + PAD
  local content_clip = {x = origin_x, y = origin_y, width = self.content_width, height = self.content_height}
  if only ~= nil then
    only:layout(origin_x, origin_y, content_clip)
    return
  end
  for _, item in ipairs(self.attached) do
    item:layout(origin_x, origin_y, content_clip)
  end
end

local function limits(self)
  local visible = math.min(16 * P, self.outer_width)
  local low_x = visible - self.outer_width
  local high_x = floor_grid(viewport.width - visible)
  if high_x < low_x then
    high_x = low_x
  end
  local high_y = math.max(0, floor_grid(viewport.height - self.title_height))
  return low_x, high_x, high_y
end

local function clamp_position(self, x, y)
  local low_x, high_x, high_y = limits(self)
  return clamp(ui.snap(x), low_x, high_x), clamp(ui.snap(y), 0, high_y)
end

local function dimensions(self)
  local title = font.layout(self.title)
  self.title_text_width = title.width * P
  self.title_text_height = title.height * P
  self.title_height = (title.height + 4) * P
  local close_width = 0
  if not self.fixed then
    close_width = select(1, button.measure("x"))
  end
  self.close_width = close_width
  local title_width = TITLE_X + self.title_text_width + 4 * P
  if not self.fixed then
    title_width = title_width + close_width + P
  end
  self.outer_width = math.max(
    self.content_width + 2 * PAD,
    title_width,
    self.fixed and 17 * P or 0
  )
  self.outer_height = self.title_height + PAD + self.content_height + BOTTOM
end

local function publish_chrome(self)
  self.frame:set_rect(0, 0, self.outer_width, self.outer_height)
  self.title_bar:set_rect(0, 0, self.outer_width, self.title_height)
  if self.divider ~= nil then
    render.set_rect(self.divider, P, self.title_height, self.outer_width - 2 * P, P)
  end
  local title_hit_width = self.fixed and self.outer_width or self.outer_width - self.close_width - P
  self.title_line:move(TITLE_X, (self.title_height - self.title_text_height) / 2)
  render.set_hit(self.frame_key, {
    {x = 0, y = 0, width = self.outer_width, height = self.outer_height},
  }, self.chrome)
  render.set_hit(self.title_key, {
    {x = 0, y = 0, width = title_hit_width, height = self.title_height},
  }, self.chrome)
  if self.close_button ~= nil then
    self.close_button:move(self.outer_width - self.close_width - P, 0)
  end
  render.set_group_clip(self.content, {
    x = self.x + PAD,
    y = self.y + self.title_height + PAD,
    width = self.content_width,
    height = self.content_height,
  })
end

local function set_position(self, x, y)
  self.x, self.y = clamp_position(self, x, y)
  render.set_group_offset(self.chrome, self.x, self.y)
  render.set_group_offset(self.content, self.x + PAD, self.y + self.title_height + PAD)
  -- Keep the content clip in the same published UI callback as both offsets.
  render.set_group_clip(self.content, {
    x = self.x + PAD,
    y = self.y + self.title_height + PAD,
    width = self.content_width,
    height = self.content_height,
  })
  layout_items(self)
end

local function raise(self)
  if self.closed then
    return
  end
  render.raise_groups(stack(self))
  for index, candidate in ipairs(windows) do
    if candidate == self then
      table.remove(windows, index)
      break
    end
  end
  windows[#windows + 1] = self
end

function window.new(spec)
  spec = spec or {}
  counter = counter + 1
  local self = setmetatable({
    id = counter,
    title = spec.title or "",
    content_width = ui.snap_size(spec.content_width or P),
    content_height = ui.snap_size(spec.content_height or P),
    x = spec.x,
    y = spec.y,
    kind = spec.kind or "window",
    fixed = spec.fixed == true,
    on_close = spec.on_close,
    on_title_click = spec.on_title_click,
    closed = false,
    attached = {},
    _pointer_children = {},
  }, Window)
  dimensions(self)
  if self.x == nil then
    self.x = (viewport.width - self.outer_width) / 2
  end
  if self.y == nil then
    self.y = (viewport.height - self.outer_height) / 2
  end
  self.x, self.y = clamp_position(self, self.x, self.y)
  self.chrome = render.create_group()
  self.content = render.create_group()
  local colours = theme.kind(self.kind)
  self.colours = colours
  self.frame = frame.new{
    x = 0,
    y = 0,
    width = self.outer_width,
    height = self.outer_height,
    cut = CUT,
    corners = "tl_br",
    fill = colours.fill,
    line = colours.outline,
    group = self.chrome,
  }
  self.title_bar = frame.new{
    x = 0,
    y = 0,
    width = self.outer_width,
    height = self.title_height,
    cut = CUT,
    corners = "tl",
    fill = colours.outline,
    line = colours.outline,
    group = self.chrome,
  }
  if colours.divider ~= nil then
    self.divider = render.create_quad{
      texture = ui.texture("ui/theme/white.png"),
      x = P,
      y = self.title_height,
      width = self.outer_width - 2 * P,
      height = P,
      group = self.chrome,
      tint = colours.divider,
    }
  end
  self.title_line = label.new{x = TITLE_X, y = 0, text = self.title, group = self.chrome, colour = colours.title_text}
  self.frame_key = ui.key("window_frame")
  self.title_key = ui.key("window_title")
  render.set_hit(self.frame_key, {{x = 0, y = 0, width = self.outer_width, height = self.outer_height}}, self.chrome)
  pointer.register(self.frame_key, self, self)
  render.set_hit(self.title_key, {{x = 0, y = 0, width = self.outer_width, height = self.title_height}}, self.chrome)
  pointer.register(self.title_key, self, self)
  if not self.fixed then
    self.close_button = button.new{
      x = self.outer_width - self.close_width - P,
      y = 0,
      text = "x",
      kind = self.kind,
      style = "title",
      group = self.chrome,
      owner = self,
      on_click = function() self:close() end,
    }
  end
  publish_chrome(self)
  render.set_group_offset(self.chrome, self.x, self.y)
  render.set_group_offset(self.content, self.x + PAD, self.y + self.title_height + PAD)
  render.raise_groups{self.chrome, self.content}
  windows[#windows + 1] = self
  return self
end

function Window:on_child_down()
  raise(self)
end

function Window:on_event(event, eligible)
  if self.closed then
    return
  end
  if eligible == false then
    return
  end
  if event.kind == "down" and event.key == self.title_key then
    if self.fixed then
      self.title_pressed = true
    else
      self.drag = {x = event.x, y = event.y, left = self.x, top = self.y}
    end
    pointer.pressed(self.title_key, true)
  elseif event.kind == "up" and self.fixed and event.key == self.title_key then
    local clicked = self.title_pressed and event.inside == true
    self.title_pressed = false
    pointer.pressed(self.title_key, false)
    if clicked and self.on_title_click ~= nil then
      self.on_title_click(self)
    end
  elseif (event.kind == "move" or event.kind == "up") and self.drag ~= nil then
    set_position(self, self.drag.left + event.x - self.drag.x, self.drag.top + event.y - self.drag.y)
    if event.kind == "up" then
      self.drag = nil
      pointer.pressed(self.title_key, false)
    end
  elseif event.kind == "cancel" then
    self.drag = nil
    self.title_pressed = false
    pointer.pressed(self.title_key, false)
  end
end

function Window:on_hover() end
function Window:on_overflow()
  self.drag = nil
  self.title_pressed = false
  pointer.pressed(self.title_key, false)
end

function Window:raise()
  raise(self)
end

-- Attaches an item (`groups`, `layout(origin_x, origin_y, content_clip)`, `destroy()`): it is laid out now and after every change of
-- the window's position or content rectangle, its groups are drawn above the content in attach order, and it is destroyed when the
-- window closes. Windows keep their own order: every window's stack is raised again, bottom first.
function Window:attach(item)
  if self.closed then
    error("window:attach: the window is closed", 2)
  end
  self.attached[#self.attached + 1] = item
  layout_items(self, item)
  local groups = {}
  for _, candidate in ipairs(windows) do
    for _, group in ipairs(stack(candidate)) do
      groups[#groups + 1] = group
    end
  end
  render.raise_groups(groups)
end

-- Removes an item that destroyed itself.
function Window:detach(item)
  for index, candidate in ipairs(self.attached) do
    if candidate == item then
      table.remove(self.attached, index)
      return
    end
  end
end

function Window:move_to(x, y)
  if not self.closed then
    set_position(self, x, y)
  end
end

function Window:position()
  return self.x, self.y
end

function Window:size()
  return self.outer_width, self.outer_height
end

function Window:content_origin()
  return self.x + PAD, self.y + self.title_height + PAD
end

function Window:set_title(title)
  if self.closed then return end
  self.title = title
  dimensions(self)
  self.title_line:set_text(title)
  publish_chrome(self)
  set_position(self, self.x, self.y)
end

function Window:set_content_size(width, height)
  if self.closed then return end
  self.content_width = ui.snap_size(width)
  self.content_height = ui.snap_size(height)
  dimensions(self)
  publish_chrome(self)
  set_position(self, self.x, self.y)
end

function Window:close()
  if self.closed then return end
  self.closed = true
  self.drag = nil
  self.title_pressed = false
  pointer.unregister(self.frame_key)
  pointer.unregister(self.title_key)
  pointer.unregister_owner(self)
  render.set_hit(self.frame_key, nil, self.chrome)
  render.set_hit(self.title_key, nil, self.chrome)
  -- The items go before the window's own groups: each removes its group, its keys and its registrations.
  local attached = self.attached
  self.attached = {}
  for _, item in ipairs(attached) do
    item:destroy()
  end
  local chrome, content = self.chrome, self.content
  self.chrome, self.content = nil, nil
  render.destroy_group(chrome)
  render.destroy_group(content)
  for index, candidate in ipairs(windows) do
    if candidate == self then
      table.remove(windows, index)
      break
    end
  end
  if self.on_close ~= nil then
    self.on_close(self)
  end
end

function window.top()
  return windows[#windows]
end

function window.close_request(source)
  if source == "key" and window.top() ~= nil then
    window.top():close()
    return false
  end
  return true
end

function window.update_viewport(width, height)
  viewport.width = width
  viewport.height = height
  for _, current in ipairs(windows) do
    set_position(current, current.x, current.y)
  end
end

return window
