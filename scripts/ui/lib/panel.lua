-- A panel is one stretched quad.
local ui = require("lib.ui")

local panel = {}
panel.__index = panel

-- `panel.new{x, y, width, height, texture = "ui/font/panel.png", group = nil}`, in points. With a `group` the quad goes into it and the
-- position is local to the group (the engine applies the group's offset).
function panel.new(spec)
  local self = setmetatable({}, panel)
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.width = ui.snap_size(spec.width)
  self.height = ui.snap_size(spec.height)
  self.quad = render.create_quad{
    texture = ui.texture(spec.texture or "ui/font/panel.png"),
    x = self.x,
    y = self.y,
    width = self.width,
    height = self.height,
    group = spec.group,
  }
  return self
end

function panel:set_rect(x, y, width, height)
  self.x = ui.snap(x)
  self.y = ui.snap(y)
  self.width = ui.snap_size(width)
  self.height = ui.snap_size(height)
  render.set_rect(self.quad, self.x, self.y, self.width, self.height)
end

function panel:destroy()
  if self.quad ~= nil then
    render.destroy_quad(self.quad)
    self.quad = nil
  end
end

return panel
