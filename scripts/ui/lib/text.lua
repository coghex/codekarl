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

local function create_glyph(l, glyph, height, colour)
  return render.create_quad{
    texture = ui.texture(glyph.texture),
    x = place(l, glyph),
    y = l.y,
    width = glyph.width * P,
    height = height * P,
    group = l.group,
    tint = colour,
  }
end

local function is_digit(c)
  return c ~= nil and c >= "0" and c <= "9"
end

-- Split a string into maximal numeric and ordinary runs. A numeric run accepts grouped/decimal punctuation and one trailing percent.
function text.number_runs(value)
  if type(value) ~= "string" then error("text.number_runs: expected a string", 2) end
  local out, index = {}, 1
  while index <= #value do
    if is_digit(value:sub(index, index)) then
      local finish = index
      while is_digit(value:sub(finish + 1, finish + 1)) do finish = finish + 1 end
      while (value:sub(finish + 1, finish + 1) == "." or value:sub(finish + 1, finish + 1) == ",")
          and is_digit(value:sub(finish + 2, finish + 2)) do
        finish = finish + 2
        while is_digit(value:sub(finish + 1, finish + 1)) do finish = finish + 1 end
      end
      if value:sub(finish + 1, finish + 1) == "%" then finish = finish + 1 end
      out[#out + 1] = {text = value:sub(index, finish), number = true}
      index = finish + 1
    else
      local finish = index
      while finish <= #value and not is_digit(value:sub(finish, finish)) do finish = finish + 1 end
      out[#out + 1] = {text = value:sub(index, finish - 1), number = false}
      index = finish
    end
  end
  return out
end

function text.colour_numbers(value, colour, number_colour)
  local out = {}
  for _, run in ipairs(text.number_runs(value)) do
    out[#out + 1] = {text = run.text, colour = run.number and number_colour or colour}
  end
  return out
end

function text.colour_segments(segments, number_colour, term_colour)
  local out = {}
  for _, segment in ipairs(segments) do
    local base = segment.term and term_colour or segment.colour
    for _, run in ipairs(text.number_runs(segment.text)) do
      out[#out + 1] = {text = run.text, colour = run.number and number_colour or base}
    end
  end
  return out
end

-- An empty line whose top-left corner is the point (x, y), in points, snapped to the UI-pixel grid. Its glyph quads go into `group`
-- (a group token) when one is given, and the position is then local to the group. The glyphs are white art tinted `colour` (a
-- colour string; the theme's text colour when not given).
function text.line(x, y, group, colour, font_name)
  return {
    x = ui.snap(x),
    y = ui.snap(y),
    glyphs = {},
    width = 0,
    height = 0,
    group = group,
    colour = colour or theme.colour("text"),
    font_name = font_name or "standard",
  }
end

-- Retints every glyph of the line; glyphs created later take the colour too. The same colour makes no call.
function text.set_colour(l, colour)
  local unchanged = true
  for _, glyph in ipairs(l.glyphs) do
    if glyph.colour ~= colour then unchanged = false; break end
  end
  if colour == l.colour and unchanged then return end
  l.colour = colour
  for _, glyph in ipairs(l.glyphs) do
    if glyph.colour ~= colour then
      render.set_tint(glyph.quad, colour)
      glyph.colour = colour
    end
  end
end

local function set_fragments(l, fragments)
  local laid_glyphs, width, height = {}, 0, 0
  for _, fragment in ipairs(fragments) do
    if fragment.text ~= "" then
      local laid = font.layout(fragment.text, l.font_name)
      for _, glyph in ipairs(laid.glyphs) do
        laid_glyphs[#laid_glyphs + 1] = {glyph = glyph, x = width + glyph.x, colour = fragment.colour or l.colour}
      end
      width = width + laid.width
      height = math.max(height, laid.height)
    end
  end
  local old = l.glyphs
  local new = {}
  for index, fragment in ipairs(laid_glyphs) do
    local glyph = fragment.glyph
    local kept = old[index]
    if kept ~= nil and kept.texture == glyph.texture then
      if kept.x ~= fragment.x then
        render.set_position(kept.quad, l.x + fragment.x * P, l.y)
      end
      if kept.colour ~= fragment.colour then render.set_tint(kept.quad, fragment.colour) end
      new[index] = {texture = glyph.texture, x = fragment.x, quad = kept.quad, colour = fragment.colour}
    else
      if kept ~= nil then
        render.destroy_quad(kept.quad)
      end
      new[index] = {texture = glyph.texture, x = fragment.x, quad = create_glyph(l, glyph, height, fragment.colour), colour = fragment.colour}
    end
  end
  for index = #laid_glyphs + 1, #old do
    render.destroy_quad(old[index].quad)
  end
  l.glyphs = new
  l.width = width
  l.height = height
end

-- Lays `content` out and updates the line's quads to match, glyph by glyph in order. `width` and `height` are then the text's size
-- in font pixels (UI pixels).
function text.set(l, content)
  set_fragments(l, {{text = content, colour = l.colour}})
end

-- As `set`, but each text fragment has its own tint. Fragment widths are additive, preserving the original line positions.
function text.set_segments(l, segments)
  set_fragments(l, segments)
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
