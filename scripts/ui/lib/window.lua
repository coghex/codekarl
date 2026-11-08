-- Draggable, clipped UI windows. Geometry and the public API use window points;
-- the renderer applies each group's offset to the window-local contents.
local ui = require("lib.ui")
local panel = require("lib.panel")
local label = require("lib.label")
local button = require("lib.button")
local pointer = require("lib.pointer")

local P = ui.P
local PAD = 3 * P
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
  local close = font.layout("x")
  self.title_text_width = title.width * P
  self.title_text_height = title.height * P
  self.title_height = (title.height + 4) * P
  self.close_width = (close.width + 4) * P
  self.outer_width = math.max(
    self.content_width + 2 * PAD,
    PAD + self.title_text_width + 4 * P + self.close_width + P
  )
  self.outer_height = self.title_height + PAD + self.content_height + PAD
end

local function publish_chrome(self)
  self.frame:set_rect(0, 0, self.outer_width, self.outer_height)
  local close_x = self.outer_width - self.close_width - P
  self.title_line:move(3 * P, (self.title_height - self.title_text_height) / 2)
  render.set_hit(self.frame_key, {
    {x = 0, y = 0, width = self.outer_width, height = self.outer_height},
  }, self.chrome)
  render.set_hit(self.title_key, {
    {x = 0, y = 0, width = close_x, height = self.title_height},
  }, self.chrome)
  self.close_button:move(close_x, 0)
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
end

local function raise(self)
  if self.closed then
    return
  end
  render.raise_groups{self.chrome, self.content}
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
    on_close = spec.on_close,
    closed = false,
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
  self.frame = panel.new{x = 0, y = 0, width = self.outer_width, height = self.outer_height, group = self.chrome}
  self.title_line = label.new{x = 3 * P, y = 0, text = self.title, group = self.chrome}
  self.frame_key = ui.key("window_frame")
  self.title_key = ui.key("window_title")
  render.set_hit(self.frame_key, {{x = 0, y = 0, width = self.outer_width, height = self.outer_height}}, self.chrome)
  pointer.register(self.frame_key, self, self)
  render.set_hit(self.title_key, {{x = 0, y = 0, width = self.outer_width, height = self.title_height}}, self.chrome)
  pointer.register(self.title_key, self, self)
  self.close_button = button.new{
    x = self.outer_width - self.close_width - P,
    y = 0,
    text = "x",
    group = self.chrome,
    owner = self,
    on_click = function() self:close() end,
  }
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
    self.drag = {x = event.x, y = event.y, left = self.x, top = self.y}
    pointer.pressed(self.title_key, true)
  elseif (event.kind == "move" or event.kind == "up") and self.drag ~= nil then
    set_position(self, self.drag.left + event.x - self.drag.x, self.drag.top + event.y - self.drag.y)
    if event.kind == "up" then
      self.drag = nil
      pointer.pressed(self.title_key, false)
    end
  elseif event.kind == "cancel" then
    self.drag = nil
    pointer.pressed(self.title_key, false)
  end
end

function Window:on_hover() end
function Window:on_overflow()
  self.drag = nil
  pointer.pressed(self.title_key, false)
end

function Window:raise()
  raise(self)
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
  pointer.unregister(self.frame_key)
  pointer.unregister(self.title_key)
  pointer.unregister_owner(self)
  render.set_hit(self.frame_key, nil, self.chrome)
  render.set_hit(self.title_key, nil, self.chrome)
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
