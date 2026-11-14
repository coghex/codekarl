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
local ICON_HEIGHT = 11
local ICON_WIDTHS = {rewind = 9, fast_forward = 9, pause = 7, play = 7, stockpile = 9, yield = 9, elevation = 9,
  location = 9, pops = 9, cards = 9, rows = 9, filter = 9, sort = 9}
local TRANSPARENT = "#00000000"

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

-- The size of an icon button without creating it.
function button.measure_icon(name)
  local width = ICON_WIDTHS[name]
  if width == nil then error("button.measure_icon: unknown icon `" .. tostring(name) .. "`", 2) end
  return (width + 2 * PADDING) * P, (ICON_HEIGHT + 4) * P
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

-- The colours the state calls for (the theme's table of the design: fill, line, text), pressed over hover over normal. A selected
-- button (a tab that is showing) looks pressed until it is deselected.
local function look_of(self)
  local kind = self.colours
  local down = self.pressed or self.selected
  if self.style == "title" then
    if down then
      return kind.hot, kind.hot, kind.pressed_text
    elseif self.hovered then
      return kind.fill, kind.hot, kind.text
    end
    return kind.outline, kind.outline, kind.title_text
  end
  if down then
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
  if self.icons ~= nil then
    local current = self.icon_quads[self.icon]
    if current.tint ~= ink then
      render.set_tint(current.quad, ink)
      current.tint = ink
    end
  else
    text.set_colour(self.line, ink)
  end
end

local function checked_icons(icons)
  if type(icons) ~= "table" or #icons == 0 then
    error("button.new: icons must be a non-empty array", 3)
  end
  local count = 0
  local width = nil
  local copy = {}
  local seen = {}
  for index, name in ipairs(icons) do
    count = count + 1
    local icon_width = ICON_WIDTHS[name]
    if icon_width == nil then
      error("button.new: unknown icon `" .. tostring(name) .. "`", 3)
    end
    -- A repeated name would create a second quad that `icon_quads` (keyed by name) could never reach: `set_icon` would leave it
    -- drawn and `destroy` could not remove it. It is refused here, before any frame, quad or region exists.
    if seen[name] then
      error("button.new: icon `" .. name .. "` is named twice in icons", 3)
    end
    seen[name] = true
    if width ~= nil and width ~= icon_width then
      error("button.new: icons must all have the same width", 3)
    end
    width = icon_width
    copy[index] = name
  end
  local entries = 0
  for _ in pairs(icons) do entries = entries + 1 end
  if entries ~= count then
    error("button.new: icons must be a non-empty array", 3)
  end
  return copy, width
end

local function icon_tint(self)
  local _, _, ink = look_of(self)
  return ink
end

local function retint_icon(self, name, colour)
  local icon = self.icon_quads[name]
  if icon.tint ~= colour then
    render.set_tint(icon.quad, colour)
    icon.tint = colour
  end
end

local function release(self)
  if self.pressed then
    self.pressed = false
    pointer.pressed(self.key, false)
  end
end

-- `button.new{x, y, text, on_click, kind = "window", style = "panel", group = nil}` or `button.new{..., icons = {...}, icon = name}`.
-- With a `group` the quads and the hit key go into it and the position is local to it (the engine applies the group's offset).
-- `style = "title"` is the look on a title bar. Icons are all created up front and `:set_icon` only retints them.
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
  local icon_width = nil
  if spec.icons ~= nil then
    self.icons, icon_width = checked_icons(spec.icons)
    self.icon = spec.icon or self.icons[1]
    local found = false
    for _, name in ipairs(self.icons) do
      if name == self.icon then found = true end
    end
    if not found then
      error("button.new: icon must be named in icons", 2)
    end
    self.width = (icon_width + 2 * PADDING) * P
    self.height = (ICON_HEIGHT + 4) * P
    self.text_x = 0
  else
    if spec.icon ~= nil then
      error("button.new: icon requires icons", 2)
    end
    local measured = font.layout(spec.text or "")
    resize(self, measured.width, measured.height)
  end
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
  if spec.tooltip ~= nil then
    self:set_tooltip(spec.tooltip)
  end
  if self.icons ~= nil then
    self.icon_quads = {}
    for _, name in ipairs(self.icons) do
      local width = ICON_WIDTHS[name]
      local colour = name == self.icon and ink or TRANSPARENT
      self.icon_quads[name] = {
        quad = render.create_quad{
          texture = ui.texture("ui/theme/icon_" .. name .. ".png"),
          x = self.x + ((icon_width + 2 * PADDING - width) // 2) * P,
          y = self.y + 2 * P,
          width = width * P,
          height = ICON_HEIGHT * P,
          group = self.group,
          tint = colour,
        },
        tint = colour,
        width = width,
      }
    end
  else
    self.line = text.line(self.x + self.text_x, self.y + PADDING * P, self.group, ink)
    text.set(self.line, spec.text or "")
  end
  publish_region(self)
  pointer.register(self.key, self, self.owner)
  return self
end

-- Sets or clears this button's one-line hover label.
function button:set_tooltip(content)
  if self.frame == nil then return end
  if content == nil then
    if self.tooltip_attached then require("lib.tooltip").detach(self.key) end
    self.tooltip_attached = false
    return
  end
  if type(content) ~= "string" then
    error("button.set_tooltip: expected a string or nil", 2)
  end
  require("lib.tooltip").label(self.key, content)
  self.tooltip_attached = true
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

function button:functional()
  return self.on_click ~= nil
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
  if self.icons ~= nil then
    for name, icon in pairs(self.icon_quads) do
      render.set_position(icon.quad, self.x + ((self.width // P - icon.width) // 2) * P, self.y + 2 * P)
    end
  else
    text.move(self.line, self.x + self.text_x, self.y + PADDING * P)
  end
  publish_region(self)
end

-- Marks the button as selected (it keeps the pressed look) or not.
function button:set_selected(selected)
  selected = selected == true
  if self.selected == selected then
    return
  end
  self.selected = selected
  show(self)
end

-- Sets the text, resizes the background and resets the region.
function button:set_text(content)
  if self.icons ~= nil then
    error("button.set_text: icon buttons do not have text", 2)
  end
  text.set(self.line, content)
  resize(self, self.line.width, self.line.height)
  self.frame:set_rect(self.x, self.y, self.width, self.height)
  text.move(self.line, self.x + self.text_x, self.y + PADDING * P)
  publish_region(self)
end

-- Changes the shown icon by retinting its already-created quad and the next one; same-icon calls do nothing.
function button:set_icon(name)
  if self.icons == nil then
    error("button.set_icon: this button has no icons", 2)
  end
  if self.icon_quads[name] == nil then
    error("button.set_icon: icon `" .. tostring(name) .. "` is not in icons", 2)
  end
  if name == self.icon then
    return
  end
  retint_icon(self, self.icon, TRANSPARENT)
  retint_icon(self, name, icon_tint(self))
  self.icon = name
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
  if self.tooltip_attached then require("lib.tooltip").detach(self.key) end
  render.set_hit(self.key, nil, self.group)
  self.frame:destroy()
  if self.line ~= nil then
    text.destroy(self.line)
  end
  if self.icon_quads ~= nil then
    for _, icon in pairs(self.icon_quads) do
      render.destroy_quad(icon.quad)
    end
  end
  self.frame = nil
end

return button
