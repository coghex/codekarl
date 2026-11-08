-- A line of text is a row of glyph quads: `font.layout` says which glyph goes where, and `set` keeps a quad when its glyph and
-- position are unchanged, moves it when only its position changed and replaces it when its glyph changed, so a readout whose
-- digits change touches one quad (CK-197's reuse rule).
local ui = require("lib.ui")
local theme = require("lib.theme")

local P = ui.P
local text = {}

local function place(l, glyph)
  return l.x + glyph.x * P
end

local function create_glyph(l, glyph, height)
  return render.create_quad{
    texture = ui.texture(glyph.texture),
    x = place(l, glyph),
    y = l.y,
    width = glyph.width * P,
    height = height * P,
    group = l.group,
    tint = l.colour,
  }
end

-- An empty line whose top-left corner is the point (x, y), in points, snapped to the UI-pixel grid. Its glyph quads go into `group`
-- (a group token) when one is given, and the position is then local to the group. The glyphs are white art tinted `colour` (a
-- colour string; the theme's text colour when not given).
function text.line(x, y, group, colour)
  return {
    x = ui.snap(x),
    y = ui.snap(y),
    glyphs = {},
    width = 0,
    height = 0,
    group = group,
    colour = colour or theme.colour("text"),
  }
end

-- Retints every glyph of the line; glyphs created later take the colour too. The same colour makes no call.
function text.set_colour(l, colour)
  if colour == l.colour then
    return
  end
  l.colour = colour
  for _, glyph in ipairs(l.glyphs) do
    render.set_tint(glyph.quad, colour)
  end
end

-- Lays `content` out and updates the line's quads to match, glyph by glyph in order. `width` and `height` are then the text's size
-- in font pixels (UI pixels).
function text.set(l, content)
  local laid = font.layout(content)
  local old = l.glyphs
  local new = {}
  for index, glyph in ipairs(laid.glyphs) do
    local kept = old[index]
    if kept ~= nil and kept.texture == glyph.texture then
      if kept.x ~= glyph.x then
        render.set_position(kept.quad, place(l, glyph), l.y)
      end
      new[index] = {texture = glyph.texture, x = glyph.x, quad = kept.quad}
    else
      if kept ~= nil then
        render.destroy_quad(kept.quad)
      end
      new[index] = {texture = glyph.texture, x = glyph.x, quad = create_glyph(l, glyph, laid.height)}
    end
  end
  for index = #laid.glyphs + 1, #old do
    render.destroy_quad(old[index].quad)
  end
  l.glyphs = new
  l.width = laid.width
  l.height = laid.height
end

-- Moves the line's corner to (x, y) and repositions every quad; the same (snapped) position makes no call.
function text.move(l, x, y)
  x = ui.snap(x)
  y = ui.snap(y)
  if l.x == x and l.y == y then
    return
  end
  l.x = x
  l.y = y
  for _, glyph in ipairs(l.glyphs) do
    render.set_position(glyph.quad, place(l, glyph), l.y)
  end
end

function text.destroy(l)
  for _, glyph in ipairs(l.glyphs) do
    render.destroy_quad(glyph.quad)
  end
  l.glyphs = {}
  l.width = 0
end

return text
