-- A button: three background quads (normal, hover, pressed; only the current one is in place), a label drawn above them and a hit
-- region under its own key. Its state comes from the pointer events routed to that key.
local ui = require("lib.ui")
local text = require("lib.text")
local pointer = require("lib.pointer")

local P = ui.P
local PADDING = 2 -- UI pixels on each side of the text
local TEXTURES = {
  normal = "ui/button/normal.png",
  hover = "ui/button/hover.png",
  pressed = "ui/button/pressed.png",
}

local button = {}
button.__index = button

-- The size from the text's size in font pixels (UI pixels): the text plus the padding on each side, in points.
local function resize(self, text_width, text_height)
  self.width = (text_width + 2 * PADDING) * P
  self.height = (text_height + 2 * PADDING) * P
end

local function publish_region(self)
  render.set_hit(self.key, {{x = self.x, y = self.y, width = self.width, height = self.height}}, self.group)
end

-- The look the state calls for: pressed while pressed, hover while hovered and not pressed, normal otherwise. A look that is
-- already current makes no scene call.
local function show(self)
  local look = "normal"
  if self.pressed then
    look = "pressed"
  elseif self.hovered then
    look = "hover"
  end
  if look == self.look then
    return
  end
  render.set_position(self.quads[self.look], ui.HIDDEN, ui.HIDDEN)
  render.set_position(self.quads[look], self.x, self.y)
  self.look = look
end

local function release(self)
  if self.pressed then
    self.pressed = false
    pointer.pressed(self.key, false)
  end
end

-- `button.new{x, y, text, on_click, group = nil}`, in points. With a `group` the quads and the hit key go into it and the position is
-- local to it (the engine applies the group's offset).
function button.new(spec)
  local self = setmetatable({}, button)
  self.on_click = spec.on_click
  self.group = spec.group
  self.owner = spec.owner
  self.key = ui.key("button")
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.hovered = false
  self.pressed = false
  self.look = "normal"
  -- Measured before the quads are created (so they have their size); the glyphs are created after them, so they are on top.
  self.line = text.line(self.x + PADDING * P, self.y + PADDING * P, self.group)
  local measured = font.layout(spec.text or "")
  resize(self, measured.width, measured.height)
  self.quads = {}
  for _, name in ipairs({"normal", "hover", "pressed"}) do
    local parked = name ~= "normal"
    self.quads[name] = render.create_quad{
      texture = ui.texture(TEXTURES[name]),
      x = parked and ui.HIDDEN or self.x,
      y = parked and ui.HIDDEN or self.y,
      width = self.width,
      height = self.height,
      group = self.group,
    }
  end
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
  render.set_position(self.quads[self.look], self.x, self.y)
  text.move(self.line, self.x + PADDING * P, self.y + PADDING * P)
  publish_region(self)
end

-- Sets the text, resizes the background and resets the region.
function button:set_text(content)
  text.set(self.line, content)
  resize(self, self.line.width, self.line.height)
  for name, quad in pairs(self.quads) do
    if name == self.look then
      render.set_rect(quad, self.x, self.y, self.width, self.height)
    else
      render.set_rect(quad, ui.HIDDEN, ui.HIDDEN, self.width, self.height)
    end
  end
  publish_region(self)
end

-- The size in points.
function button:size()
  return self.width, self.height
end

function button:destroy()
  if self.quads == nil then
    return
  end
  pointer.unregister(self.key)
  render.set_hit(self.key, nil, self.group)
  for _, name in ipairs({"normal", "hover", "pressed"}) do
    render.destroy_quad(self.quads[name])
  end
  text.destroy(self.line)
  self.quads = nil
end

return button
