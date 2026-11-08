-- A button: three background quads (normal, hover, pressed; only the current one is in place), a label drawn above them and a hit
-- region under its own key. Its state comes from the pointer events routed to that key.
local ui = require("lib.ui")
local text = require("lib.text")
local frame = require("lib.frame")
local theme = require("lib.theme")
local pointer = require("lib.pointer")

local P = ui.P
local PADDING = 2 -- UI pixels on each side of the text
local CUT = 3
local LEAST = 2 * CUT + 3 -- the least size of a cut-3 frame, in UI pixels

local button = {}
button.__index = button

-- The size in UI pixels from the text's size in font pixels: the text plus the padding on each side, at least the frame's minimum.
local function size_of(text_width, text_height)
  return math.max(text_width + 2 * PADDING, LEAST), math.max(text_height + 2 * PADDING, LEAST)
end

-- The size in points of a button with `content` as its text.
function button.measure(content)
  local laid = font.layout(content or "")
  local width, height = size_of(laid.width, laid.height)
  return width * P, height * P
end

-- A text narrower than the button is centred: its left edge, in UI pixels from the button's.
local function text_offset(width_px, text_width)
  return PADDING + (width_px - 2 * PADDING - text_width) // 2
end

local function resize(self, text_width, text_height)
  local width, height = size_of(text_width, text_height)
  self.width = width * P
  self.height = height * P
  self.text_x = text_offset(width, text_width) * P
end

local function publish_region(self)
  render.set_hit(self.key, {{x = self.x, y = self.y, width = self.width, height = self.height}}, self.group)
end

-- The colours the state calls for (the theme's table of the design: fill, line, text), pressed over hover over normal.
local function look_of(self)
  local kind = self.colours
  if self.style == "title" then
    if self.pressed then
      return kind.hot, kind.hot, kind.pressed_text
    elseif self.hovered then
      return kind.fill, kind.hot, kind.text
    end
    return kind.outline, kind.outline, kind.title_text
  end
  if self.pressed then
    return kind.hot, kind.hot, kind.pressed_text
  elseif self.hovered then
    return kind.fill, kind.hot, kind.text
  end
  return kind.fill, kind.outline, kind.text
end

-- Shows the look the state calls for. A colour that is already current makes no scene call.
local function show(self)
  local fill, line, ink = look_of(self)
  self.frame:set_colours(fill, line)
  text.set_colour(self.line, ink)
end

local function release(self)
  if self.pressed then
    self.pressed = false
    pointer.pressed(self.key, false)
  end
end

-- `button.new{x, y, text, on_click, kind = "window", style = "panel", group = nil}`, in points. With a `group` the quads and the hit
-- key go into it and the position is local to it (the engine applies the group's offset). `style = "title"` is the look on a
-- title bar.
function button.new(spec)
  local self = setmetatable({}, button)
  self.on_click = spec.on_click
  self.group = spec.group
  self.owner = spec.owner
  self.style = spec.style or "panel"
  self.colours = theme.kind(spec.kind or "window")
  self.key = ui.key("button")
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.hovered = false
  self.pressed = false
  local measured = font.layout(spec.text or "")
  resize(self, measured.width, measured.height)
  local fill, line, ink = look_of(self)
  -- The frame is created before the glyphs, so they are on top.
  self.frame = frame.new{
    x = self.x,
    y = self.y,
    width = self.width,
    height = self.height,
    cut = CUT,
    corners = "tl_br",
    fill = fill,
    line = line,
    group = self.group,
  }
  self.line = text.line(self.x + self.text_x, self.y + PADDING * P, self.group, ink)
  text.set(self.line, spec.text or "")
  publish_region(self)
  pointer.register(self.key, self, self.owner)
  return self
end

function button:on_event(event, eligible)
  local kind = event.kind
  if kind == "down" then
    if not self.pressed then
      self.pressed = true
      pointer.pressed(self.key, true)
      show(self)
    end
  elseif kind == "up" then
    if self.pressed then
      release(self)
      show(self)
      if eligible ~= false and event.inside == true and self.on_click ~= nil then
        self.on_click()
      end
    end
  elseif kind == "cancel" then
    release(self)
    show(self)
  end
end

function button:on_hover(is_hovered)
  self.hovered = is_hovered
  show(self)
end

function button:on_overflow()
  release(self)
  show(self)
end

function button:move(x, y)
  self.x = ui.snap(x)
  self.y = ui.snap(y)
  self.frame:set_rect(self.x, self.y, self.width, self.height)
  text.move(self.line, self.x + self.text_x, self.y + PADDING * P)
  publish_region(self)
end

-- Sets the text, resizes the background and resets the region.
function button:set_text(content)
  text.set(self.line, content)
  resize(self, self.line.width, self.line.height)
  self.frame:set_rect(self.x, self.y, self.width, self.height)
  text.move(self.line, self.x + self.text_x, self.y + PADDING * P)
  publish_region(self)
end

-- The size in points.
function button:size()
  return self.width, self.height
end

function button:destroy()
  if self.frame == nil then
    return
  end
  pointer.unregister(self.key)
  render.set_hit(self.key, nil, self.group)
  self.frame:destroy()
  text.destroy(self.line)
  self.frame = nil
end

return button
