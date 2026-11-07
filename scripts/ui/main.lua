-- The ui VM: the common demo. It opens an 800 by 600 window with a solid clear colour, logs when the
-- window is open, and logs the source of a close request (Escape or the close button) and accepts it.
-- Run headless (--headless, or no [window] backend), `window.open` is ignored with one record and the rest
-- of the script runs unchanged.

-- The pixel font and the date readout (with `[game] scenario` only: that is when `font` and `game` exist). The UI VM has no
-- module loading, so the text helpers are local functions here. A line is a row of glyph quads: `font.layout` says which glyph
-- goes where, and `set` keeps a quad when its glyph and position are unchanged, moves it when only its position changed and
-- replaces it when its glyph changed, so a readout whose digits change touches one quad.
local FONT_POINTS_PER_PIXEL = (config.ui and config.ui.font_points_per_pixel) or 2
local MARGIN = 8 -- points from the window's top and left edges

-- Each texture is loaded once, on first use, and kept for the run.
local textures = {}
local function texture_token(name)
  local token = textures[name]
  if token == nil then
    token = texture.load(name)
    textures[name] = token
  end
  return token
end

-- An empty line whose top-left corner is the point (x, y).
local function line(x, y)
  return {x = x, y = y, glyphs = {}, width = 0, height = 0}
end

local function place(l, glyph)
  return l.x + glyph.x * FONT_POINTS_PER_PIXEL
end

local function create_glyph(l, glyph, height)
  return render.create_quad{
    texture = texture_token(glyph.texture),
    x = place(l, glyph),
    y = l.y,
    width = glyph.width * FONT_POINTS_PER_PIXEL,
    height = height * FONT_POINTS_PER_PIXEL,
  }
end

-- Lays `text` out and updates the line's quads to match, glyph by glyph in order.
local function set(l, text)
  local laid = font.layout(text)
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

-- Moves the line's corner to (x, y) and repositions every quad; the same position makes no call.
local function move(l, x, y)
  if l.x == x and l.y == y then
    return
  end
  l.x = x
  l.y = y
  for _, glyph in ipairs(l.glyphs) do
    render.set_position(glyph.quad, place(l, glyph), l.y)
  end
end

local function destroy(l)
  for _, glyph in ipairs(l.glyphs) do
    render.destroy_quad(glyph.quad)
  end
  l.glyphs = {}
  l.width = 0
end

-- The readout's text for a `game_time` table: `9 Oct 768 · 14:00 · speed 3`, or `... · paused` whatever the reason.
local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
local function readout_text(time)
  local state = time.paused and "paused" or ("speed " .. time.speed)
  return string.format("%d %s %d · %02d:00 · %s", time.day, MONTHS[time.month], time.year, time.hour, state)
end

if game ~= nil then
  local readout = line(MARGIN + FONT_POINTS_PER_PIXEL, MARGIN + FONT_POINTS_PER_PIXEL)
  local panel = nil
  game.on_time(function(time)
    local text = readout_text(time)
    if panel == nil then
      -- The panel is created first, so that the glyphs are drawn over it.
      local laid = font.layout(text)
      panel = render.create_quad{
        texture = texture_token("ui/font/panel.png"),
        x = MARGIN,
        y = MARGIN,
        width = (laid.width + 2) * FONT_POINTS_PER_PIXEL,
        height = (laid.height + 2) * FONT_POINTS_PER_PIXEL,
      }
    end
    local before = readout.width
    set(readout, text)
    if readout.width ~= before then
      render.set_rect(panel, MARGIN, MARGIN, (readout.width + 2) * FONT_POINTS_PER_PIXEL, (readout.height + 2) * FONT_POINTS_PER_PIXEL)
    end
  end)

  local tooltip = line(0, 0)
  local tooltip_panel = nil
  local tooltip_id = nil
  local tooltip_visible = false
  local WINDOW_WIDTH = 800
  local HIDDEN = -1000000
  game.on_hover(function(hover)
    if hover == nil then
      if tooltip_visible then
        move(tooltip, HIDDEN, HIDDEN)
        if tooltip_panel ~= nil then
          render.set_rect(tooltip_panel, HIDDEN, HIDDEN,
            (tooltip.width + 2) * FONT_POINTS_PER_PIXEL,
            (tooltip.height + 2) * FONT_POINTS_PER_PIXEL)
        end
        tooltip_visible = false
      end
      return
    end

    local text_changed = tooltip_id ~= hover.id
    local measured = text_changed and font.layout(hover.name) or nil
    local padding = FONT_POINTS_PER_PIXEL
    local width = measured and measured.width or tooltip.width
    local height = measured and measured.height or tooltip.height
    local panel_width = (width + 2) * FONT_POINTS_PER_PIXEL
    local panel_height = (height + 2) * FONT_POINTS_PER_PIXEL
    local panel_x = hover.right + 4
    if panel_x + panel_width > WINDOW_WIDTH and hover.left - 4 - panel_width >= 0 then
      panel_x = hover.left - 4 - panel_width
    end
    local panel_y = hover.y - panel_height / 2
    if tooltip_panel == nil then
      tooltip_panel = render.create_quad{
        texture = texture_token("ui/font/panel.png"),
        x = panel_x,
        y = panel_y,
        width = panel_width,
        height = panel_height,
      }
      if text_changed then
        set(tooltip, hover.name)
      end
      tooltip_id = hover.id
    else
      if text_changed then
        set(tooltip, hover.name)
      end
      tooltip_id = hover.id
      render.set_rect(tooltip_panel, panel_x, panel_y, panel_width, panel_height)
    end
    move(tooltip, panel_x + padding, panel_y + padding)
    tooltip_visible = true
  end)
end

print("ui: loaded")
log.info("loaded")

window.open{title = "Codekarl", width = 800, height = 600, clear = {0.10, 0.25, 0.45}}

window.on_opened(function()
  log.info("the window is open")
end)

window.on_close_request(function(source)
  log.info("close requested by " .. source)
  return true
end)

function startup()
  print("ui: startup")
  log.info("startup")
end

function shutdown()
  print("ui: shutdown")
  log.info("shutdown")
end
