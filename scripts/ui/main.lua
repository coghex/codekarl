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
local lists = require("lib.list")
local theme = require("lib.theme")
local date_text = require("lib.time")
local location = require("lib.location")

local P = ui.P
local MARGIN = 8 -- points from the window's top and left edges

-- The window as opened, and then as the last update reported it, in points.
local OPEN_WIDTH, OPEN_HEIGHT = 800, 600
local view = {width = OPEN_WIDTH, height = OPEN_HEIGHT}

-- Relayouts of the anchored elements, run when the window's size changes. Each is registered by the element.
local relayouts = {}
local help_window = nil
local places_window = nil

if game ~= nil then
  -- The time bar is created with the first `game_time`: fixed transport controls and speed boxes, then the changing date.
  local time_colours = theme.kind("time_bar")
  local readout = text.line(0, 0, nil, time_colours.text)
  local time_panel = nil
  local readout_x = nil
  local places_panel = nil
  local places_button = nil
  local places_group = nil
  local places_bar_visible = true
  local modes_panel = nil
  local modes_group = nil
  local mode_buttons = {}
  local shown_mode = "off"
  local speed_boxes = {}
  local time_controls = {}
  local shown_speed = nil
  local shown_paused = nil

  -- The panel follows only the date width. The control positions and readout origin stay fixed.
  local function fit_bar()
    local width = readout_x - time_panel.x + readout.width * P + 5 * P
    local height = 19 * P
    local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
    time_panel:set_rect(x, y, width, height)
    render.set_hit("time_bar", {{x = time_panel.x, y = time_panel.y, width = time_panel.width, height = time_panel.height}})
    text.move(readout, readout_x, y + 4 * P)
  end

  -- Defined below, with the Places window.
  local open_places

  local function help_button(x, y)
    local help_width, help_height = button.measure("?")
    return button.new{
      x = x,
      y = y,
      text = "?",
      kind = "time_bar",
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
  end

  local function fit_places_bar()
    if places_panel == nil then return end
    local x, y = ui.anchor("top_right", places_panel.width, places_panel.height, -MARGIN, MARGIN, view.width, view.height)
    places_panel:set_rect(x, y, places_panel.width, places_panel.height)
    render.set_hit("places_bar", {{x = x, y = y, width = places_panel.width, height = places_panel.height}}, places_group)
    places_button:move(x + 5 * P, y + 2 * P)
  end

  local function set_places_bar_visible(visible)
    if places_group ~= nil and places_bar_visible ~= visible then
      render.set_group_visible(places_group, visible)
      places_bar_visible = visible
    end
  end

  local function create_places_bar()
    local places_width, places_height = button.measure("places")
    local width, height = 5 * P + places_width + 5 * P, 19 * P
    local x, y = ui.anchor("top_right", width, height, -MARGIN, MARGIN, view.width, view.height)
    places_group = render.create_group()
    places_panel = panel.new{
      x = x, y = y, width = width, height = height, kind = "places_bar", cut = 6, group = places_group,
    }
    -- Publish the bar capture before the button's narrower region.
    render.set_hit("places_bar", {{x = x, y = y, width = width, height = height}}, places_group)
    places_button = button.new{
      x = x + 5 * P,
      y = y + 2 * P,
      text = "places",
      kind = "places_bar",
      group = places_group,
      on_click = function() open_places() end,
    }
  end

  -- The map modes' bar (milestone 9, slice 8): two icon buttons, left of the places bar, choose the map mode that colours each place red
  -- to green: the goods in store against the most it holds, or the year's yield against the other places'. The button of the mode
  -- that shows is selected; pressing it turns the mode off, pressing the other changes the mode.
  local MODE_BUTTON_WIDTH = 13 -- UI pixels: the 9-pixel icon and the button's padding
  local MODES_WIDTH = 5 + MODE_BUTTON_WIDTH + 3 + MODE_BUTTON_WIDTH + 5

  local function modes_origin()
    return ui.anchor("top_right", MODES_WIDTH * P, 19 * P, -(MARGIN + places_panel.width + 4 * P), MARGIN, view.width, view.height)
  end

  local function fit_modes_bar()
    if modes_panel == nil or places_panel == nil then return end
    local x, y = modes_origin()
    modes_panel:set_rect(x, y, MODES_WIDTH * P, 19 * P)
    render.set_hit("modes_bar", {{x = x, y = y, width = MODES_WIDTH * P, height = 19 * P}}, modes_group)
    mode_buttons.stockpile:move(x + 5 * P, y + 2 * P)
    mode_buttons.yield:move(x + (5 + MODE_BUTTON_WIDTH + 3) * P, y + 2 * P)
  end

  local function select_mode(name)
    local wanted = name == shown_mode and "off" or name
    -- A mode the map lane refuses leaves the one that showed.
    if not pcall(game.set_map_mode, wanted) then
      return
    end
    shown_mode = wanted
    for key, mode_button in pairs(mode_buttons) do
      mode_button:set_selected(key == shown_mode)
    end
  end

  local function create_modes_bar()
    local x, y = modes_origin()
    modes_group = render.create_group()
    modes_panel = panel.new{
      x = x, y = y, width = MODES_WIDTH * P, height = 19 * P, kind = "places_bar", cut = 6, group = modes_group,
    }
    -- Publish the bar capture before the buttons' narrower regions.
    render.set_hit("modes_bar", {{x = x, y = y, width = MODES_WIDTH * P, height = 19 * P}}, modes_group)
    mode_buttons.stockpile = button.new{
      x = x + 5 * P, y = y + 2 * P, icons = {"stockpile"}, icon = "stockpile", kind = "places_bar", group = modes_group,
      on_click = function() select_mode("stockpile") end,
    }
    mode_buttons.yield = button.new{
      x = x + (5 + MODE_BUTTON_WIDTH + 3) * P, y = y + 2 * P, icons = {"yield"}, icon = "yield", kind = "places_bar",
      group = modes_group,
      on_click = function() select_mode("yield") end,
    }
  end

  local function update_speed_box(box)
    local outline = time_colours.speed_box
    if box.hovered or box.pressed then outline = time_colours.hot end
    if outline ~= box.outline_tint then
      render.set_tint(box.outline, outline)
      box.outline_tint = outline
    end
    local filled = box.index <= shown_speed or box.pressed
    local inner = filled and time_colours.speed_fill or "#00000000"
    if inner ~= box.inner_tint then
      render.set_tint(box.inner, inner)
      box.inner_tint = inner
    end
  end

  local function speed_box_widget(index, key)
    local box = {index = index, key = key, hovered = false, pressed = false}
    function box:on_event(event, eligible)
      if event.kind == "down" then
        if not self.pressed then
          self.pressed = true
          pointer.pressed(self.key, true)
          update_speed_box(self)
        end
      elseif event.kind == "up" then
        if self.pressed then
          self.pressed = false
          pointer.pressed(self.key, false)
          update_speed_box(self)
          if eligible ~= false and event.inside == true then
            game.control("time_speed_" .. self.index)
          end
        end
      elseif event.kind == "cancel" then
        self.pressed = false
        pointer.pressed(self.key, false)
        update_speed_box(self)
      end
    end
    function box:on_hover(hovered)
      self.hovered = hovered
      update_speed_box(self)
    end
    function box:on_overflow()
      self.pressed = false
      pointer.pressed(self.key, false)
      update_speed_box(self)
    end
    return box
  end

  local function update_speed(time)
    shown_speed = time.speed
    for index, box in ipairs(speed_boxes) do
      update_speed_box(box)
    end
  end

  local function create_bar(time, content)
    local help_width = select(1, button.measure("?"))
    local date_left = 90 + help_width / P + 4
    local width = (date_left + font.layout(content).width + 5) * P
    local height = 19 * P
    local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
    -- The panel and broad capture region precede all button regions.
    time_panel = panel.new{x = x, y = y, width = width, height = height, kind = "time_bar", cut = 6}
    render.set_hit("time_bar", {{x = x, y = y, width = width, height = height}})
    for index = 1, 5 do
      local box_x = x + (48 + 8 * (index - 1)) * P
      local outline = render.create_quad{
        texture = ui.texture("ui/theme/speed_box.png"),
        x = box_x, y = y + 4 * P, width = 6 * P, height = 11 * P,
        tint = time_colours.speed_box,
      }
      local filled = index <= time.speed
      local inner_tint = filled and time_colours.speed_fill or "#00000000"
      local inner = render.create_quad{
        texture = ui.texture("ui/theme/white.png"),
        x = box_x + 2 * P, y = y + 6 * P, width = 2 * P, height = 7 * P,
        tint = inner_tint,
      }
      local key = "speed_" .. index
      local widget = speed_box_widget(index, key)
      widget.outline, widget.outline_tint = outline, time_colours.speed_box
      widget.inner, widget.inner_tint = inner, inner_tint
      speed_boxes[index] = widget
      render.set_hit(key, {{x = x + (47 + 8 * (index - 1)) * P, y = y + 2 * P, width = 8 * P, height = 15 * P}})
      pointer.register(key, widget)
    end

    local row_y = y + 2 * P
    button.new{x = x + 5 * P, y = row_y, icons = {"rewind"}, icon = "rewind", kind = "time_bar",
      on_click = function() game.control("time_slower") end}
    time_controls.pause = button.new{x = x + 19 * P, y = row_y, icons = {"pause", "play"},
      icon = time.paused and "play" or "pause", kind = "time_bar",
      on_click = function() game.control("time_pause") end}
    button.new{x = x + 31 * P, y = row_y, icons = {"fast_forward"}, icon = "fast_forward", kind = "time_bar",
      on_click = function() game.control("time_faster") end}

    shown_speed = time.speed
    shown_paused = not not time.paused

    help_button(x + 90 * P, row_y)
    readout_x = x + date_left * P
    text.set(readout, content)
    text.move(readout, readout_x, y + 4 * P)
    create_places_bar()
    create_modes_bar()
  end

  -- The location panel: the left third of the screen below the time bar, filled from the economy view of the revision `game_time`
  -- carries (`game.place`), never from live state.
  local location_panel = location.new{
    view = view,
    top = function()
      if time_panel ~= nil then
        return time_panel.y + time_panel.height + MARGIN
      end
      return 0
    end,
  }

  game.on_time(function(time)
    location_panel:set_revision(time.economy_revision)
    local content = date_text.date(time)
    if time_panel == nil then
      create_bar(time, content)
      return
    end
    local before = readout.width
    text.set(readout, content)
    if readout.width ~= before then
      fit_bar()
    end
    if time.speed ~= shown_speed then update_speed(time) end
    local paused = not not time.paused
    if paused ~= shown_paused then
      -- The second button key is stable: rewind is first and the pause/play control is second.
      -- Keep the button object rather than routing state changes through a scene-wide rebuild.
      time_controls.pause:set_icon(paused and "play" or "pause")
      shown_paused = paused
    end
  end)
  relayouts[#relayouts + 1] = function()
    if time_panel ~= nil then
      fit_bar()
    end
  end
  relayouts[#relayouts + 1] = fit_places_bar
  relayouts[#relayouts + 1] = fit_modes_bar

  -- A place's `id` and `name` (`game_select`'s fields, a map click's or a places-list row's) open the location panel, or show that
  -- place in it. The places list only selects.
  local function show_place(selection)
    location_panel:show(selection)
  end

  relayouts[#relayouts + 1] = function()
    location_panel:relayout()
  end

  game.on_select(show_place)

  -- The places dropdown: every location in file order. A row opens the place popup and leaves this list open.
  open_places = function()
    if places_window ~= nil and not places_window.closed then
      places_window:raise()
      return
    end
    local places = game.places()
    local names = {}
    local widest = 0
    for index, place in ipairs(places) do
      names[index] = place.name
      widest = math.max(widest, font.layout(place.name).width)
    end
    local list_width = (widest + 2) * P
    local title_width = font.layout("places").width * P
    local outer_width = math.max(list_width + 6 * P, 12 * P + title_width, 17 * P)
    local created
    created = windows.new{
      title = "places",
      fixed = true,
      content_width = list_width,
      content_height = 12 * lists.ROW,
      kind = "places_bar",
      x = 0,
      y = 0,
      on_title_click = function() created:close() end,
      on_close = function()
        if places_window == created then
          places_window = nil
          set_places_bar_visible(true)
        end
      end,
    }
    local width, height = created:size()
    local x, y = ui.anchor("top_right", width, height, -MARGIN, MARGIN, view.width, view.height)
    created:move_to(x, y)
    lists.new{
      window = created,
      x = 0,
      y = 0,
      width = list_width,
      rows = 12,
      items = names,
      on_select = function(index) show_place(places[index]) end,
    }
    places_window = created
    set_places_bar_visible(false)
  end

  relayouts[#relayouts + 1] = function()
    if places_window ~= nil and not places_window.closed then
      local width, height = places_window:size()
      local x, y = ui.anchor("top_right", width, height, -MARGIN, MARGIN, view.width, view.height)
      places_window:move_to(x, y)
    end
  end

  -- The tooltip: a panel and a text line beside the hovered place, on the side that fits the window's current width.
  local tooltip_colours = theme.kind("tooltip")
  local tooltip = text.line(0, 0, nil, tooltip_colours.text)
  local tooltip_panel = nil
  local tooltip_id = nil
  local tooltip_visible = false
  local last_hover = nil

  local function show_tooltip(hover)
    local text_changed = tooltip_id ~= hover.id
    local measured = text_changed and font.layout(hover.name) or nil
    local width = measured and measured.width or tooltip.width
    local height = measured and measured.height or tooltip.height
    local panel_width = math.max(width + 4, 9) * P
    local panel_height = (height + 4) * P
    local panel_x = hover.right + 4
    if panel_x + panel_width > view.width and hover.left - 4 - panel_width >= 0 then
      panel_x = hover.left - 4 - panel_width
    end
    local panel_y = hover.y - panel_height / 2
    if tooltip_panel == nil then
      tooltip_panel = panel.new{x = panel_x, y = panel_y, width = panel_width, height = panel_height, kind = "tooltip", cut = 3}
    else
      tooltip_panel:set_rect(panel_x, panel_y, panel_width, panel_height)
    end
    if text_changed then
      text.set(tooltip, hover.name)
    end
    tooltip_id = hover.id
    text.move(tooltip, tooltip_panel.x + 2 * P, tooltip_panel.y + 2 * P)
    tooltip_visible = true
  end

  game.on_hover(function(hover)
    last_hover = hover
    if hover == nil then
      if tooltip_visible then
        text.move(tooltip, ui.HIDDEN, ui.HIDDEN)
        if tooltip_panel ~= nil then
          tooltip_panel:set_rect(ui.HIDDEN, ui.HIDDEN, math.max(tooltip.width + 4, 9) * P, (tooltip.height + 4) * P)
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
