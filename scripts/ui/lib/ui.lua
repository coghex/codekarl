-- The widget library's shared part (milestone 8, slice 2; docs/ui-design.md §3.3).
--
-- Every number the library takes or returns is in window points. `P` is the UI pixel in points (`[ui] points_per_pixel`): the font is
-- drawn at P points per font pixel, and a widget snaps what it is given to the grid of whole UI pixels, once, where the value enters
-- the widget.
local ui = {}

local P = (config.ui and config.ui.points_per_pixel) or 2
ui.P = P

-- The nearest whole UI pixel, halves up.
function ui.snap(value)
  return math.floor(value / P + 0.5) * P
end

-- A parking coordinate: far outside any window, on the UI-pixel grid like every other edge.
ui.HIDDEN = ui.snap(-1000000)

-- A size snapped to the grid, at least one UI pixel.
function ui.snap_size(value)
  local snapped = ui.snap(value)
  if snapped < P then
    return P
  end
  return snapped
end

-- Each texture is loaded once, on first use, and kept for the run.
local textures = {}
function ui.texture(name)
  local token = textures[name]
  if token == nil then
    token = texture.load(name)
    textures[name] = token
  end
  return token
end

-- A hit key `<prefix>_<n>` that no other call has handed out: the host takes `[a-z0-9_]{1,48}`.
local counter = 0
function ui.key(prefix)
  if type(prefix) ~= "string" or not string.find(prefix, "^[a-z0-9_]+$") then
    error("ui.key: the prefix must be lowercase letters, digits and '_'", 2)
  end
  counter = counter + 1
  local key = prefix .. "_" .. counter
  if #key > 48 then
    error("ui.key: `" .. key .. "` is longer than 48 characters", 2)
  end
  return key
end

-- Where along one axis a widget of `size` points sits in a window of `window` points: the near edge, the middle or the far edge,
-- moved by `offset`. The result is in points and not yet snapped.
local function along(side, size, window, offset)
  if side == "near" then
    return offset
  elseif side == "middle" then
    return (window - size) / 2 + offset
  end
  return window - size + offset
end

local ANCHORS = {
  top_left = {"near", "near"},
  top = {"middle", "near"},
  top_right = {"far", "near"},
  left = {"near", "middle"},
  centre = {"middle", "middle"},
  right = {"far", "middle"},
  bottom_left = {"near", "far"},
  bottom = {"middle", "far"},
  bottom_right = {"far", "far"},
}

-- The top-left corner, snapped, of a widget `width` by `height` points (its own snapped size) anchored to `name` in a window of
-- `window_width` by `window_height` points, moved by (`dx`, `dy`).
function ui.anchor(name, width, height, dx, dy, window_width, window_height)
  local sides = ANCHORS[name]
  if sides == nil then
    error("ui.anchor: `" .. tostring(name) .. "` is not an anchor", 2)
  end
  dx = dx or 0
  dy = dy or 0
  return ui.snap(along(sides[1], width, window_width, dx)), ui.snap(along(sides[2], height, window_height, dy))
end

return ui
