-- A label is a text line with no background.
local ui = require("lib.ui")
local text = require("lib.text")

local label = {}
label.__index = label

-- `label.new{x, y, text, group = nil, colour = nil}`, in points; with a `group` the glyphs go into it and the position is local to
-- it. The text is tinted `colour` (the theme's text colour by default).
function label.new(spec)
  local self = setmetatable({}, label)
  self.owner = spec.owner
  self.line = text.line(spec.x, spec.y, spec.group, spec.colour)
  text.set(self.line, spec.text or "")
  return self
end

function label:set_text(content)
  text.set(self.line, content)
end

function label:set_colour(colour)
  text.set_colour(self.line, colour)
end

function label:move(x, y)
  text.move(self.line, x, y)
end

-- The text's size in points, a multiple of the UI pixel.
function label:size()
  return self.line.width * ui.P, self.line.height * ui.P
end

function label:destroy()
  text.destroy(self.line)
end

return label
