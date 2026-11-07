-- The ui VM: the common demo. It opens an 800 by 600 window with a solid clear colour, logs when the
-- window is open, and logs the source of a close request (Escape or the close button) and accepts it.
-- Run headless (--headless, or no [window] backend), `window.open` is ignored with one record and the rest
-- of the script runs unchanged.
--
-- The widget library is in `lib/` (`require` loads modules from beneath this script's directory only). The date readout and the
-- hover tooltip (with `[game] scenario` only: that is when `game` exists) are built from it, and `frame.on_update` keeps their
-- layout against the window's current size and gives the pointer events to the widgets.

local ui = require("lib.ui")
local text = require("lib.text")
local panel = require("lib.panel")
local pointer = require("lib.pointer")

local P = ui.P
local MARGIN = 8 -- points from the window's top and left edges

-- The window as opened, and then as the last update reported it, in points.
local OPEN_WIDTH, OPEN_HEIGHT = 800, 600
local view = {width = OPEN_WIDTH, height = OPEN_HEIGHT}

-- Relayouts of the anchored elements, run when the window's size changes. Each is registered by the element.
local relayouts = {}

-- The readout's text for a `game_time` table: `9 Oct 768 · 14:00 · speed 3`, or `... · paused` whatever the reason.
local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
local function readout_text(time)
  local state = time.paused and "paused" or ("speed " .. time.speed)
  return string.format("%d %s %d · %02d:00 · %s", time.day, MONTHS[time.month], time.year, time.hour, state)
end

if game ~= nil then
  -- The readout: a panel one UI pixel larger than the text on each side, anchored to the top-left corner. It is created with the
  -- first `game_time`, the panel first so that the glyphs are drawn over it.
  local readout = text.line(0, 0)
  local readout_panel = nil

  -- Places the panel, the hit region and the text for the readout's current text size.
  local function fit_readout()
    local width = (readout.width + 2) * P
    local height = (readout.height + 2) * P
    local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
    readout_panel:set_rect(x, y, width, height)
    render.set_hit("readout", {{x = readout_panel.x, y = readout_panel.y, width = readout_panel.width, height = readout_panel.height}})
    text.move(readout, readout_panel.x + P, readout_panel.y + P)
  end

  game.on_time(function(time)
    local content = readout_text(time)
    if readout_panel == nil then
      local laid = font.layout(content)
      local width = (laid.width + 2) * P
      local height = (laid.height + 2) * P
      local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
      readout_panel = panel.new{x = x, y = y, width = width, height = height}
      render.set_hit("readout", {{x = readout_panel.x, y = readout_panel.y, width = readout_panel.width, height = readout_panel.height}})
      text.move(readout, readout_panel.x + P, readout_panel.y + P)
    end
    local before = readout.width
    text.set(readout, content)
    if readout.width ~= before then
      fit_readout()
    end
  end)
  relayouts[#relayouts + 1] = function()
    if readout_panel ~= nil then
      fit_readout()
    end
  end

  -- The tooltip: a panel and a text line beside the hovered place, on the side that fits the window's current width.
  local tooltip = text.line(0, 0)
  local tooltip_panel = nil
  local tooltip_id = nil
  local tooltip_visible = false
  local last_hover = nil

  local function show_tooltip(hover)
    local text_changed = tooltip_id ~= hover.id
    local measured = text_changed and font.layout(hover.name) or nil
    local width = measured and measured.width or tooltip.width
    local height = measured and measured.height or tooltip.height
    local panel_width = (width + 2) * P
    local panel_height = (height + 2) * P
    local panel_x = hover.right + 4
    if panel_x + panel_width > view.width and hover.left - 4 - panel_width >= 0 then
      panel_x = hover.left - 4 - panel_width
    end
    local panel_y = hover.y - panel_height / 2
    if tooltip_panel == nil then
      tooltip_panel = panel.new{x = panel_x, y = panel_y, width = panel_width, height = panel_height}
    else
      tooltip_panel:set_rect(panel_x, panel_y, panel_width, panel_height)
    end
    if text_changed then
      text.set(tooltip, hover.name)
    end
    tooltip_id = hover.id
    text.move(tooltip, tooltip_panel.x + P, tooltip_panel.y + P)
    tooltip_visible = true
  end

  game.on_hover(function(hover)
    last_hover = hover
    if hover == nil then
      if tooltip_visible then
        text.move(tooltip, ui.HIDDEN, ui.HIDDEN)
        if tooltip_panel ~= nil then
          tooltip_panel:set_rect(ui.HIDDEN, ui.HIDDEN, (tooltip.width + 2) * P, (tooltip.height + 2) * P)
        end
        tooltip_visible = false
      end
      return
    end
    show_tooltip(hover)
  end)
  relayouts[#relayouts + 1] = function()
    if tooltip_visible and last_hover ~= nil then
      show_tooltip(last_hover)
    end
  end
end

-- Every update gives the pointer to the dispatcher (which makes scene calls only for a change a widget shows) and, when the window's
-- size changed, relayouts the anchored elements. An update with an unchanged size and nothing for the dispatcher makes no scene call.
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  if update.width ~= view.width or update.height ~= view.height then
    view.width = update.width
    view.height = update.height
    for _, relayout in ipairs(relayouts) do
      relayout()
    end
  end
end)

print("ui: loaded")
log.info("loaded")

window.open{title = "Codekarl", width = OPEN_WIDTH, height = OPEN_HEIGHT, clear = {0.10, 0.25, 0.45}}

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
