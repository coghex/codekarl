-- A label is a text line with no background.
local ui = require("lib.ui")
local text = require("lib.text")

local label = {}
label.__index = label

-- `label.new{x, y, text}`, in points.
function label.new(spec)
  local self = setmetatable({}, label)
  self.line = text.line(spec.x, spec.y)
  text.set(self.line, spec.text or "")
  return self
end

function label:set_text(content)
  text.set(self.line, content)
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
