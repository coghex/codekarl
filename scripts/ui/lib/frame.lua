-- A cut-corner frame (milestone 8, slice 7b; docs/ui-design.md §3.7): a fill and a one-pixel outline whose top-left corner, and
-- optionally the bottom-right one, is cut at 45 degrees. It is a fixed set of tinted quads (`ui/theme/*.png` tiles at the cut corners,
-- stretched `white.png` bands and edges), so a colour change is a retint and a size change moves the quads.
--
-- `frame.new{x, y, width, height, cut, corners, fill, line, group}`: points, snapped; `cut` is 3, 6 or 7; `corners` is "tl_br" or "tl";
-- `fill` and `line` are colours. The result has `:set_rect(x, y, width, height)`, `:set_colours(fill, line)` and `:destroy()`.
local ui = require("lib.ui")

local P = ui.P
local frame = {}
frame.__index = frame

local WHITE = "ui/theme/white.png"

local function tile(cut, corner, part)
  return string.format("ui/theme/cut%d_%s_%s.png", cut, corner, part)
end

-- The quads of a frame `w` by `h` UI pixels, in painter order, each `{texture, role, x, y, width, height}` in UI pixels from the
-- frame's top-left pixel. Ranges are inclusive in the design and become a position and a size here.
local function parts(w, h, c, tl, br)
  local list = {}
  local function add(texture, role, x1, y1, x2, y2)
    list[#list + 1] = {texture = texture, role = role, x = x1, y = y1, width = x2 - x1 + 1, height = y2 - y1 + 1}
  end
  if tl then
    add(tile(c, "tl", "fill"), "fill", 0, 0, c, c)
  end
  if br then
    add(tile(c, "br", "fill"), "fill", w - 1 - c, h - 1 - c, w - 1, h - 1)
  end
  if tl then
    add(WHITE, "fill", c + 1, 1, w - 2, c)
  end
  add(WHITE, "fill", 1, tl and c + 1 or 1, w - 2, br and h - 2 - c or h - 2)
  if br then
    add(WHITE, "fill", 1, h - 1 - c, w - 2 - c, h - 2)
  end
  if tl then
    add(tile(c, "tl", "line"), "line", 0, 0, c, c)
  end
  if br then
    add(tile(c, "br", "line"), "line", w - 1 - c, h - 1 - c, w - 1, h - 1)
  end
  add(WHITE, "line", tl and c + 1 or 0, 0, w - 1, 0)
  add(WHITE, "line", w - 1, 1, w - 1, br and h - 2 - c or h - 1)
  add(WHITE, "line", 1, h - 1, br and w - 2 - c or w - 2, h - 1)
  add(WHITE, "line", 0, tl and c + 1 or 1, 0, h - 1)
  return list
end

-- The quads' rectangles in points for the frame's current position and size.
local function place(self)
  local list = parts(self.width // P, self.height // P, self.cut, true, self.both)
  for _, part in ipairs(list) do
    part.x = self.x + part.x * P
    part.y = self.y + part.y * P
    part.width = part.width * P
    part.height = part.height * P
  end
  return list
end

local function check_size(width, height, cut, both)
  local least = both and 2 * cut + 3 or cut + 3
  if width < least or height < least then
    error(string.format("frame: %dx%d UI pixels is below the %dx%d minimum of a cut-%d frame", width, height, least, least, cut), 3)
  end
end

function frame.new(spec)
  local cut = spec.cut
  if cut ~= 3 and cut ~= 6 and cut ~= 7 then
    error("frame.new: cut must be 3, 6 or 7", 2)
  end
  local both
  if spec.corners == "tl_br" then
    both = true
  elseif spec.corners == "tl" then
    both = false
  else
    error('frame.new: corners must be "tl_br" or "tl"', 2)
  end
  local self = setmetatable({}, frame)
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.width = ui.snap_size(spec.width)
  self.height = ui.snap_size(spec.height)
  self.cut = cut
  self.both = both
  check_size(self.width // P, self.height // P, cut, both)
  self.fill = spec.fill
  self.line = spec.line
  self.group = spec.group
  self.quads = {}
  for index, part in ipairs(place(self)) do
    self.quads[index] = {
      role = part.role,
      x = part.x,
      y = part.y,
      width = part.width,
      height = part.height,
      quad = render.create_quad{
        texture = ui.texture(part.texture),
        x = part.x,
        y = part.y,
        width = part.width,
        height = part.height,
        group = self.group,
        tint = part.role == "fill" and self.fill or self.line,
      },
    }
  end
  return self
end

-- Moves and resizes the frame; a quad whose rectangle is unchanged makes no call.
function frame:set_rect(x, y, width, height)
  local new_x, new_y = ui.snap(x), ui.snap(y)
  local new_width, new_height = ui.snap_size(width), ui.snap_size(height)
  check_size(new_width // P, new_height // P, self.cut, self.both)
  self.x, self.y, self.width, self.height = new_x, new_y, new_width, new_height
  for index, part in ipairs(place(self)) do
    local kept = self.quads[index]
    if kept.x ~= part.x or kept.y ~= part.y or kept.width ~= part.width or kept.height ~= part.height then
      kept.x, kept.y, kept.width, kept.height = part.x, part.y, part.width, part.height
      render.set_rect(kept.quad, part.x, part.y, part.width, part.height)
    end
  end
end

-- Changes the colours; only the quads whose colour changes are retinted.
function frame:set_colours(fill, line)
  for _, kept in ipairs(self.quads) do
    local want = kept.role == "fill" and fill or line
    local have = kept.role == "fill" and self.fill or self.line
    if want ~= have then
      render.set_tint(kept.quad, want)
    end
  end
  self.fill = fill
  self.line = line
end

function frame:destroy()
  if self.quads == nil then
    return
  end
  for _, kept in ipairs(self.quads) do
    render.destroy_quad(kept.quad)
  end
  self.quads = nil
end

return frame
