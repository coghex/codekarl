-- Drawing helpers for the character cards and the character screen (CK-375). A *view* collects the objects one draw made and destroys them
-- together; the helpers add to it. Every position is in UI pixels from the origin of `group`, every size too, except where a name says points.
local ui = require("lib.ui")
local text = require("lib.text")
local theme = require("lib.theme")
local frame = require("lib.frame")

local P = ui.P
local draw = {}

function draw.view()
  local self = {objects = {}}
  function self:add(object)
    self.objects[#self.objects + 1] = object
    return object
  end
  function self:destroy()
    for index = #self.objects, 1, -1 do
      local object = self.objects[index]
      if object.destroy ~= nil then object:destroy() end
      self.objects[index] = nil
    end
  end
  return self
end

-- A filled rectangle of one colour. A rectangle with no area (a bar at 0) is nothing to draw: the renderer refuses a quad with no width.
function draw.rect(result, group, x, y, width, height, colour)
  if width <= 0 or height <= 0 then return nil end
  local quad = render.create_quad{
    texture = ui.texture("ui/theme/white.png"), x = x * P, y = y * P, width = width * P, height = height * P,
    group = group, tint = colour,
  }
  return result:add({destroy = function() render.destroy_quad(quad) end})
end

-- An image of the character art (`ui/characters/<name>.png`), `width` by `height` UI pixels, drawn as it is or tinted `colour`.
function draw.image(result, group, name, x, y, width, height, colour)
  local quad = render.create_quad{
    texture = ui.texture("ui/characters/" .. name .. ".png"), x = x * P, y = y * P, width = width * P, height = height * P,
    group = group, tint = colour,
  }
  return result:add({destroy = function() render.destroy_quad(quad) end})
end

-- A cut-corner frame of the cut 3 filled with `fill` and lined with `line`.
function draw.frame(result, group, x, y, width, height, fill, line)
  local made = frame.new{
    x = x * P, y = y * P, width = width * P, height = height * P, cut = 3, corners = "tl_br", fill = fill, line = line, group = group,
  }
  return result:add(made)
end

-- A line of text whose top-left corner is (x, y). `content` is a string, drawn in `colour` with its numbers in the number colour, or a
-- list of `{text, colour}` segments. `font_name` is "standard" (the default), "small" or "tiny"; `align` "right" puts the line's right edge
-- at x. Returns the width in UI pixels.
function draw.text(result, group, x, y, content, colour, font_name, align)
  local base = colour or theme.colour("text")
  local line = text.line(0, 0, group, base, font_name)
  if type(content) == "string" then
    text.set_segments(line, text.colour_numbers(content, base, theme.colour("number")))
  else
    text.set_segments(line, content)
  end
  if align == "right" then x = x - line.width end
  text.move(line, x * P, y * P)
  result:add({destroy = function() text.destroy(line) end})
  return line.width
end

-- The width in UI pixels of `content` in the font `font_name`.
function draw.width(content, font_name)
  return font.layout(content, font_name or "standard").width
end

-- `content` cut to end in two dots when it is wider than `width` UI pixels (whole code points, as the ledger's columns are).
function draw.clip(content, width, font_name)
  if width <= 0 then return "" end
  if draw.width(content, font_name) <= width then return content end
  if draw.width("..", font_name) > width then return "" end
  local count = utf8.len(content) or 0
  while count > 0 do
    local boundary = utf8.offset(content, count + 1)
    local prefix = boundary ~= nil and content:sub(1, boundary - 1) or content
    local candidate = prefix .. ".."
    if draw.width(candidate, font_name) <= width then return candidate end
    count = count - 1
  end
  return ".."
end

return draw
