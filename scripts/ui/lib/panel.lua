-- A panel is a cut-corner frame (lib/frame.lua) in the colours of a kind of the theme.
local ui = require("lib.ui")
local frame = require("lib.frame")
local theme = require("lib.theme")

local panel = {}
panel.__index = panel

-- `panel.new{x, y, width, height, kind = "window", cut = 3, group = nil}`, in points. With a `group` the quads go into it and the
-- position is local to the group (the engine applies the group's offset).
function panel.new(spec)
  local self = setmetatable({}, panel)
  local colours = theme.kind(spec.kind or "window")
  self.owner = spec.owner
  self.frame = frame.new{
    x = spec.x,
    y = spec.y,
    width = spec.width,
    height = spec.height,
    cut = spec.cut or 3,
    corners = "tl_br",
    fill = colours.fill,
    line = colours.outline,
    group = spec.group,
  }
  self.x, self.y, self.width, self.height = self.frame.x, self.frame.y, self.frame.width, self.frame.height
  return self
end

function panel:set_rect(x, y, width, height)
  self.frame:set_rect(x, y, width, height)
  self.x, self.y, self.width, self.height = self.frame.x, self.frame.y, self.frame.width, self.frame.height
end

function panel:destroy()
  if self.frame ~= nil then
    self.frame:destroy()
    self.frame = nil
  end
end

return panel
