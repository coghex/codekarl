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
local label = require("lib.label")
local pointer = require("lib.pointer")
local button = require("lib.button")
local windows = require("lib.window")

local P = ui.P
local MARGIN = 8 -- points from the window's top and left edges

-- The window as opened, and then as the last update reported it, in points.
local OPEN_WIDTH, OPEN_HEIGHT = 800, 600
local view = {width = OPEN_WIDTH, height = OPEN_HEIGHT}

-- Relayouts of the anchored elements, run when the window's size changes. Each is registered by the element.
local relayouts = {}
local place_window = nil
local help_window = nil

-- The readout's text for a `game_time` table: `9 Oct 768 · 14:00 · speed 3`, or `... · paused` whatever the reason.
local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
local function readout_text(time)
  local state = time.paused and "paused" or ("speed " .. time.speed)
  return string.format("%d %s %d · %02d:00 · %s", time.day, MONTHS[time.month], time.year, time.hour, state)
end

if game ~= nil then
  -- The time bar is created with the first `game_time`: one panel behind a row of controls and the changing readout.
  local readout = text.line(0, 0)
  local time_panel = nil
  local readout_x = nil
  local row_y = nil
  local row_height = nil

  -- The panel follows only the readout width. The button positions and readout origin are fixed after first layout.
  local function fit_bar()
    local width = readout_x - time_panel.x + readout.width * P + 2 * P
    local height = row_height + 4 * P
    local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
    time_panel:set_rect(x, y, width, height)
    render.set_hit("time_bar", {{x = time_panel.x, y = time_panel.y, width = time_panel.width, height = time_panel.height}})
    text.move(readout, readout_x, row_y + (row_height - readout.height * P) / 2)
  end

  local function create_bar(content)
    local groups = {
      {{text = "||", name = "time_pause"}},
      {{text = "-", name = "time_slower"}, {text = "+", name = "time_faster"}},
      {
        {text = "1", name = "time_speed_1"},
        {text = "2", name = "time_speed_2"},
        {text = "3", name = "time_speed_3"},
        {text = "4", name = "time_speed_4"},
        {text = "5", name = "time_speed_5"},
      },
    }
    local button_height = 0
    local content_width = 0
    for group_index, group in ipairs(groups) do
      if group_index > 1 then
        content_width = content_width + 4 * P
      end
      for index, item in ipairs(group) do
        local laid = font.layout(item.text)
        item.width = (laid.width + 4) * P
        item.height = (laid.height + 4) * P
        button_height = math.max(button_height, item.height)
        content_width = content_width + item.width
        if index > 1 then
          content_width = content_width + P
        end
      end
    end
    local laid_readout = font.layout(content)
    local help_layout = font.layout("?")
    local help_width = (help_layout.width + 4) * P
    local help_height = (help_layout.height + 4) * P
    local readout_width = laid_readout.width * P
    local readout_height = laid_readout.height * P
    row_height = button_height
    local width = content_width + help_width + readout_width + 12 * P
    local height = row_height + 4 * P
    local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
    -- The panel and its broad capture region precede all button regions in painter order.
    time_panel = panel.new{x = x, y = y, width = width, height = height}
    render.set_hit("time_bar", {{x = x, y = y, width = width, height = height}})
    local cursor = x + 2 * P
    row_y = y + 2 * P
    for group_index, group in ipairs(groups) do
      if group_index > 1 then
        cursor = cursor + 4 * P
      end
      for index, item in ipairs(group) do
        if index > 1 then
          cursor = cursor + P
        end
        local control_name = item.name
        button.new{
          x = cursor,
          y = row_y + (row_height - item.height) / 2,
          text = item.text,
          on_click = function() game.control(control_name) end,
        }
        cursor = cursor + item.width
      end
    end
    button.new{
      x = cursor + 4 * P,
      y = row_y + (row_height - help_height) / 2,
      text = "?",
      on_click = function()
        if help_window ~= nil then
          help_window:raise()
          return
        end
        local lines = {
          "drag: pan",
          "scroll: zoom",
          "click a place: details",
          "space: pause",
          "1-5: speed",
          "esc: close window",
        }
        local width_px = 0
        local height_px = 0
        for _, line in ipairs(lines) do
          local size = font.layout(line)
          width_px = math.max(width_px, size.width)
          height_px = height_px + size.height
        end
        height_px = height_px + (#lines - 1) * 2
        local created
        created = windows.new{
          title = "Help",
          content_width = width_px * P,
          content_height = height_px * P,
          on_close = function()
            if help_window == created then help_window = nil end
          end,
        }
        local y = 0
        for _, line in ipairs(lines) do
          local item = label.new{x = 0, y = y, text = line, group = created.content, owner = created}
          local item_height = select(2, item:size())
          y = y + item_height + 2 * P
        end
        help_window = created
      end,
    }
    readout_x = cursor + 8 * P + help_width
    text.set(readout, content)
    text.move(readout, readout_x, row_y + (row_height - readout_height) / 2)
  end

  game.on_time(function(time)
    local content = readout_text(time)
    if time_panel == nil then
      create_bar(content)
      return
    end
    local before = readout.width
    text.set(readout, content)
    if readout.width ~= before then
      fit_bar()
    end
  end)
  relayouts[#relayouts + 1] = function()
    if time_panel ~= nil then
      fit_bar()
    end
  end

  game.on_select(function(selection)
    local lines = {
      "id: " .. selection.id,
      "kind: " .. selection.kind,
      "size: " .. selection.size,
    }
    local width_px = 0
    local height_px = 0
    for _, line in ipairs(lines) do
      local size = font.layout(line)
      width_px = math.max(width_px, size.width)
      height_px = height_px + size.height
    end
    height_px = height_px + (#lines - 1) * 2
    if place_window == nil then
      local created
      created = windows.new{
        title = selection.name,
        content_width = width_px * P,
        content_height = height_px * P,
        on_close = function()
          if place_window == created then place_window = nil end
        end,
      }
      place_window = created
    else
      place_window:set_title(selection.name)
      place_window:set_content_size(width_px * P, height_px * P)
      place_window:raise()
    end
    if place_window.lines ~= nil then
      for _, old in ipairs(place_window.lines) do old:destroy() end
    end
    place_window.lines = {}
    local y = 0
    for _, line in ipairs(lines) do
      local item = label.new{x = 0, y = y, text = line, group = place_window.content, owner = place_window}
      place_window.lines[#place_window.lines + 1] = item
      local item_height = select(2, item:size())
      y = y + item_height + 2 * P
    end
  end)

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
    windows.update_viewport(view.width, view.height)
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
  return windows.close_request(source)
end)

function startup()
  print("ui: startup")
  log.info("startup")
end

function shutdown()
  print("ui: shutdown")
  log.info("shutdown")
end
