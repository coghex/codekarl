-- The ui VM: the common demo. It opens a resizable window of 1280 by 800 screen pixels (its minimum) with a solid clear colour, logs when the
-- window is open, and logs the source of a close request (Escape or the close button). The window's own close button ends the run; Escape
-- closes what is open (a tooltip, a list-bar picker, a window) and, with nothing left, opens the pause menu (`lib/menu.lua`) and declines.
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
local listbar = require("lib.listbar")
local theme = require("lib.theme")
local date_text = require("lib.time")
local location = require("lib.location")
local tooltip = require("lib.tooltip")
local classes = require("lib.classes")
local menu = require("lib.menu")

local P = ui.P
local MARGIN = 8 -- UI units from the window's edges

-- The window opens at its minimum, in screen pixels (the owner: "for now at the minimum"; the size will be a setting later) and the user
-- can enlarge it. `window.open` returns the initial view in UI units for this pixel-sized window, which the first layout starts from; then
-- each update reports the window's size as it is.
local OPEN_WIDTH, OPEN_HEIGHT = window.open{
  title = "Codekarl", unit = "pixels", width = 1280, height = 800, resizable = true, min_width = 1280, min_height = 800,
  clear = {0.10, 0.25, 0.45},
}
local view = {width = OPEN_WIDTH, height = OPEN_HEIGHT}
windows.update_viewport(view.width, view.height)

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
  local readout_key = "date_readout"
  tooltip.attach(readout_key, function() return tooltip.explanation("date") end)
  pointer.register(readout_key, {
    on_event = function() end,
    on_hover = function() end,
    on_overflow = function() end,
  })
  local places_panel = nil
  local places_buttons = {}
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
  local fill_ledger
  local ledger_revision = nil

  -- The panel follows only the date width. The control positions and readout origin stay fixed.
  local function fit_bar()
    local width = readout_x - time_panel.x + readout.width * P + 5 * P
    local height = 19 * P
    local x, y = ui.anchor("top_left", width, height, MARGIN, MARGIN, view.width, view.height)
    time_panel:set_rect(x, y, width, height)
    render.set_hit("time_bar", {{x = time_panel.x, y = time_panel.y, width = time_panel.width, height = time_panel.height}})
    text.move(readout, readout_x, y + 4 * P)
    render.set_hit(readout_key, {{x = readout_x, y = y + 4 * P, width = readout.width * P, height = readout.height * P}})
  end

  -- Defined below, with the Places window.
  local open_ledger

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
    places_buttons.locations:move(x + 5 * P, y + 2 * P)
    places_buttons.pops:move(x + 5 * P + 16 * P, y + 2 * P)
  end

  local function set_places_bar_visible(visible)
    if places_group ~= nil and places_bar_visible ~= visible then
      render.set_group_visible(places_group, visible)
      places_bar_visible = visible
    end
  end

  local function create_places_bar()
    local width, height = 39 * P, 19 * P
    local x, y = ui.anchor("top_right", width, height, -MARGIN, MARGIN, view.width, view.height)
    places_group = ui.create_group()
    places_panel = panel.new{
      x = x, y = y, width = width, height = height, kind = "places_bar", cut = 6, group = places_group,
    }
    -- Publish the bar capture before the button's narrower region.
    render.set_hit("places_bar", {{x = x, y = y, width = width, height = height}}, places_group)
    places_buttons.locations = button.new{
      x = x + 5 * P, y = y + 2 * P, icons = {"location"}, icon = "location",
      tooltip = "places",
      kind = "places_bar",
      group = places_group,
      on_click = function() open_ledger("locations") end,
    }
    places_buttons.pops = button.new{
      x = x + 21 * P, y = y + 2 * P, icons = {"pops"}, icon = "pops",
      tooltip = "pops",
      kind = "places_bar",
      group = places_group,
      on_click = function() open_ledger("pops") end,
    }
  end

  -- The map modes' bar (milestone 9, slices 8, 10b and 12a): three icon buttons, centred at the bottom of the window, choose the map mode that colours each
  -- place red to green (the goods in store against the most it holds, or the year's yield against the other places') or draws the land
  -- from black to white by its height. The button of the mode that shows is selected; pressing it turns the mode off, pressing another
  -- changes the mode.
  local MODE_BUTTON_WIDTH = 13 -- UI pixels: the 9-pixel icon and the button's padding
  local MODES_WIDTH = 5 + MODE_BUTTON_WIDTH + 3 + MODE_BUTTON_WIDTH + 3 + MODE_BUTTON_WIDTH + 5

  -- Centred at the bottom of the window, its bottom edge a margin above the window's: it does not depend on any other panel.
  local function modes_origin()
    return ui.anchor("bottom", MODES_WIDTH * P, 19 * P, 0, -MARGIN, view.width, view.height)
  end

  local function fit_modes_bar()
    if modes_panel == nil then return end
    local x, y = modes_origin()
    modes_panel:set_rect(x, y, MODES_WIDTH * P, 19 * P)
    render.set_hit("modes_bar", {{x = x, y = y, width = MODES_WIDTH * P, height = 19 * P}}, modes_group)
    mode_buttons.stockpile:move(x + 5 * P, y + 2 * P)
    mode_buttons.yield:move(x + (5 + MODE_BUTTON_WIDTH + 3) * P, y + 2 * P)
    mode_buttons.elevation:move(x + (5 + 2 * (MODE_BUTTON_WIDTH + 3)) * P, y + 2 * P)
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
    modes_group = ui.create_group()
    modes_panel = panel.new{
      x = x, y = y, width = MODES_WIDTH * P, height = 19 * P, kind = "places_bar", cut = 6, group = modes_group,
    }
    -- Publish the bar capture before the buttons' narrower regions.
    render.set_hit("modes_bar", {{x = x, y = y, width = MODES_WIDTH * P, height = 19 * P}}, modes_group)
    mode_buttons.stockpile = button.new{
      x = x + 5 * P, y = y + 2 * P, icons = {"stockpile"}, icon = "stockpile", tooltip = "stockpile map", kind = "places_bar", group = modes_group,
      on_click = function() select_mode("stockpile") end,
    }
    mode_buttons.yield = button.new{
      x = x + (5 + MODE_BUTTON_WIDTH + 3) * P, y = y + 2 * P, icons = {"yield"}, icon = "yield", tooltip = "yield map", kind = "places_bar",
      group = modes_group,
      on_click = function() select_mode("yield") end,
    }
    mode_buttons.elevation = button.new{
      x = x + (5 + 2 * (MODE_BUTTON_WIDTH + 3)) * P, y = y + 2 * P, icons = {"elevation"}, icon = "elevation", tooltip = "elevation map", kind = "places_bar",
      group = modes_group,
      on_click = function() select_mode("elevation") end,
    }
    tooltip.attach(mode_buttons.stockpile.key, function() return tooltip.explanation("stockpile_map") end)
    tooltip.attach(mode_buttons.yield.key, function() return tooltip.explanation("yield_map") end)
    tooltip.attach(mode_buttons.elevation.key, function() return tooltip.explanation("elevation_map") end)
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
    function box:functional()
      return true
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
      tooltip.label(key, "speed " .. index)
      widget.outline, widget.outline_tint = outline, time_colours.speed_box
      widget.inner, widget.inner_tint = inner, inner_tint
      speed_boxes[index] = widget
      render.set_hit(key, {{x = x + (47 + 8 * (index - 1)) * P, y = y + 2 * P, width = 8 * P, height = 15 * P}})
      pointer.register(key, widget)
    end

    local row_y = y + 2 * P
    button.new{x = x + 5 * P, y = row_y, icons = {"rewind"}, icon = "rewind", tooltip = "slower", kind = "time_bar",
      on_click = function() game.control("time_slower") end}
    time_controls.pause = button.new{x = x + 19 * P, y = row_y, icons = {"pause", "play"},
      icon = time.paused and "play" or "pause", tooltip = time.paused and "play" or "pause", kind = "time_bar",
      on_click = function() game.control("time_pause") end}
    button.new{x = x + 31 * P, y = row_y, icons = {"fast_forward"}, icon = "fast_forward", tooltip = "faster", kind = "time_bar",
      on_click = function() game.control("time_faster") end}

    shown_speed = time.speed
    shown_paused = not not time.paused

    help_button(x + 90 * P, row_y)
    readout_x = x + date_left * P
    text.set_segments(readout, text.colour_numbers(content, theme.colour("text"), theme.colour("number")))
    text.move(readout, readout_x, y + 4 * P)
    render.set_hit(readout_key, {{x = readout_x, y = y + 4 * P, width = readout.width * P, height = readout.height * P}})
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
    classes.set_revision(time.economy_revision)
    if fill_ledger ~= nil and places_window ~= nil and not places_window.closed
      and ledger_revision ~= location_panel:current_revision() then
      fill_ledger()
    end
    local content = date_text.date(time)
    if time_panel == nil then
      create_bar(time, content)
      return
    end
    local before = readout.width
    text.set_segments(readout, text.colour_numbers(content, theme.colour("text"), theme.colour("number")))
    if readout.width ~= before then
      fit_bar()
    end
    if time.speed ~= shown_speed then update_speed(time) end
    local paused = not not time.paused
    if paused ~= shown_paused then
      -- The second button key is stable: rewind is first and the pause/play control is second.
      -- Keep the button object rather than routing state changes through a scene-wide rebuild.
      time_controls.pause:set_icon(paused and "play" or "pause")
      time_controls.pause:set_tooltip(paused and "play" or "pause")
      shown_paused = paused
    end
    tooltip.refresh()
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

  local ledger_list = nil
  local ledger_bar = nil
  local ledger_tabs = nil
  local ledger_selections = {}
  local ledger_key = "locations"
  local LEDGER_TABS = {
    {key = "locations", icon = "location", tooltip = "places"},
    {key = "pops", icon = "pops", tooltip = "pops"},
  }

  -- Long labels end before a following numeric column; use whole UTF-8 code points and the title's two-dot ellipsis rule.
  local function clip_column(value, width)
    if width <= 0 then return "" end
    if font.layout(value).width <= width then return value end
    if font.layout("..").width > width then return "" end
    local count = utf8.len(value) or 0
    while count > 0 do
      local boundary = utf8.offset(value, count + 1)
      local prefix = boundary ~= nil and value:sub(1, boundary - 1) or value
      local candidate = prefix .. ".."
      if font.layout(candidate).width <= width then return candidate end
      count = count - 1
    end
    return ".."
  end

  local function pop_colour(kind)
    local ok, colour = pcall(theme.colour, "pop_" .. tostring(kind))
    if ok and colour ~= nil then return colour end
    return theme.colour("text")
  end

  -- What the list bar of each tab sorts and filters by (CK-295): the labels are the ones the owner approved.
  local LEDGER_BARS = {
    locations = {
      sorts = {
        {key = "name", label = "name", first = "ascending"},
        {key = "people", label = "people", first = "descending"},
        {key = "food", label = "food", first = "descending"},
      },
      groups = {{key = "kind", label = "kind"}},
    },
    pops = {
      sorts = {
        {key = "type", label = "type", first = "ascending"},
        {key = "culture", label = "culture", first = "ascending"},
        {key = "people", label = "people", first = "descending"},
        {key = "place", label = "place", first = "ascending"},
      },
      groups = {{key = "type", label = "type"}, {key = "religion", label = "religion"}, {key = "culture", label = "culture"}},
    },
  }

  -- The places tab's header row and its data rows, or the pops tab's rows (no header). Each data row carries what the list bar reads
  -- (`sort`, `filter`) and the place a click on it opens (`place`); the list is given only the cells and the rest.
  local function ledger_items(key)
    local places = game.places()
    local revision = location_panel:current_revision()
    local width = places_window.content_width // P
    local header, rows = nil, {}
    if key == "locations" then
      header = {cells = {
        {text = "name", x = 1, align = "left", colour = theme.colour("term")},
        {text = "people", x = width - 37, align = "right", colour = theme.colour("term")},
        {text = "food", x = width - 3, align = "right", colour = theme.colour("term")},
      }}
      header.tooltip = function(_, _, cell)
        local explanations = {[1] = "location", [2] = "population", [3] = "food_days"}
        local explanation = explanations[cell]
        if explanation == nil then return nil end
        return tooltip.explanation(explanation), tostring(cell)
      end
      for _, place in ipairs(places) do
        local place_view = revision ~= nil and game.place(place.id, revision) or nil
        local row = {cells = {{text = clip_column(place.name, width - 72), x = 1, align = "left"}}, place = place,
          sort = {name = place.name}, filter = {kind = place.kind}}
        if place_view ~= nil and #place_view.pops > 0 then
          row.cells[2] = {text = tostring(place_view.workers + place_view.dependents), x = width - 37, align = "right"}
          row.cells[3] = {text = tostring(place_view.food_days) .. "d", x = width - 3, align = "right"}
          row.sort.people = place_view.workers + place_view.dependents
          row.sort.food = place_view.food_days
        end
        local place_id = place.id
        row.tooltip = function(_, _, cell)
          local class, args
          if cell == 1 then class, args = "location", {place = place_id}
          elseif cell == 2 and place_view ~= nil then class, args = "population", {place = place_id}
          elseif cell == 3 and place_view ~= nil then class, args = "place_food", {place = place_id}
          else return nil end
          return tooltip.content(class, args), tostring(cell)
        end
        rows[#rows + 1] = row
      end
    else
      for _, place in ipairs(places) do
        local place_view = revision ~= nil and game.place(place.id, revision) or nil
        if place_view ~= nil then
          for pop_index, pop in ipairs(place_view.pops) do
            local kind_culture = pop.kind .. " " .. pop.culture
            local row = {
              place = place,
              sort = {type = pop.kind, culture = pop.culture, people = pop.workers + pop.dependents, place = place.name},
              filter = {type = pop.kind, religion = pop.religion, culture = pop.culture},
              stripe = pop_colour(pop.kind),
              icon = {name = "pops", colour = pop_colour(pop.kind)},
              cells = {
                {text = clip_column(kind_culture, width - 120), x = 16, align = "left", colour = pop_colour(pop.kind)},
                {text = tostring(pop.workers + pop.dependents), x = width - 70, align = "right"},
                {text = clip_column(place.name, 63), x = width - 66, align = "left"},
              },
            }
            local place_id, kind, selected_pop_index = place.id, pop.kind, pop_index
            row.tooltip = function(_, _, cell)
              if cell == "icon" or cell == 1 then
                return tooltip.explanation(kind), tostring(cell)
              elseif cell == 2 then
                return tooltip.content("pop_size", {place = place_id, pop = selected_pop_index}), tostring(cell)
              elseif cell == 3 then
                return tooltip.content("location", {place = place_id}), tostring(cell)
              end
              return nil
            end
            rows[#rows + 1] = row
          end
        end
      end
    end
    return header, rows, revision
  end

  -- The bar under the ledger's list. Its choices start again with a new bar (the ledger opens, the tab changes) and stay while the
  -- bar lives, across a refresh for a new economy revision and across a change of the game window's size.
  local function ensure_ledger_bar()
    if ledger_bar ~= nil then return end
    local spec = LEDGER_BARS[ledger_key]
    ledger_bar = listbar.new{
      window = places_window,
      sorts = spec.sorts,
      groups = spec.groups,
      on_change = function() fill_ledger(true) end,
    }
  end

  -- Shows the ledger's rows through its list bar. A change of the bar's choices starts the list from its first row (`reset_scroll`); a
  -- refresh for a new revision keeps the scroll.
  fill_ledger = function(reset_scroll)
    if places_window == nil or places_window.closed then return end
    ensure_ledger_bar()
    local header, data, revision = ledger_items(ledger_key)
    local rows, selections = {}, {}
    if header ~= nil then rows[1] = header end
    for _, row in ipairs(ledger_bar:view(data)) do
      rows[#rows + 1] = row
      -- Keep the click mapping in lockstep with the rows across refreshes.
      selections[#rows] = row.place
    end
    ledger_selections = selections
    if ledger_list ~= nil and not ledger_list.closed then
      ledger_list:set_items(rows)
      if reset_scroll == true then ledger_list:set_pos(0) end
    else
      local visible_rows = math.max(1, math.min(lists.MAX_ROWS, (places_window.content_height - listbar.HEIGHT) // lists.ROW))
      ledger_list = lists.new{
        window = places_window,
        x = 0,
        y = 0,
        width = places_window.content_width,
        rows = visible_rows,
        items = rows,
        tooltips = true,
        numbers = true,
        on_select = function(index)
          local selection = ledger_selections[index]
          if selection ~= nil then
            location_panel:show(selection, ledger_key == "pops" and "pops" or nil)
          end
        end,
      }
    end
    ledger_revision = revision
  end

  local function fit_ledger()
    if places_window == nil or places_window.closed then return end
    local width = math.max(ui.snap_size(view.width // 3), ui.snap_size(location.min_width()))
    local height = math.max(ui.snap_size(view.height - MARGIN), ui.snap_size(24 * P))
    places_window:set_outer_size(width, height)
    places_window:move_to(view.width - places_window.outer_width, MARGIN)
    if ledger_list ~= nil and not ledger_list.closed then
      ledger_list:destroy()
      ledger_list = nil
    end
    fill_ledger()
  end

  local function select_ledger_tab(key)
    ledger_key = key
    if ledger_list ~= nil and not ledger_list.closed then
      ledger_list:destroy()
      ledger_list = nil
    end
    -- The other tab has other choices, and they start again.
    if ledger_bar ~= nil then
      ledger_bar:destroy()
      ledger_bar = nil
    end
    fill_ledger()
  end

  -- The ledger replaces the former places dropdown; rows open the location panel without closing this window.
  open_ledger = function(key)
    if places_window ~= nil and not places_window.closed then
      ledger_tabs:select(key)
      places_window:raise()
      return
    end
    local created
    created = windows.new{
      title = "ledger",
      clip_title = true,
      draggable = false,
      outer_width = math.max(ui.snap_size(view.width // 3), ui.snap_size(location.min_width())),
      outer_height = math.max(ui.snap_size(view.height - MARGIN), ui.snap_size(24 * P)),
      kind = "places_bar",
      x = 0,
      y = MARGIN,
      on_close = function()
        if places_window == created then
          places_window = nil
          ledger_list = nil
          ledger_bar = nil
          ledger_tabs = nil
          ledger_revision = nil
          set_places_bar_visible(true)
        end
      end,
    }
    places_window = created
    ledger_key = key
    ledger_tabs = require("lib.tabs").new{
      window = created,
      items = LEDGER_TABS,
      selected = key,
      on_select = select_ledger_tab,
    }
    created:move_to(view.width - created.outer_width, MARGIN)
    set_places_bar_visible(false)
    fill_ledger()
  end

  relayouts[#relayouts + 1] = fit_ledger

  -- The map's place name uses the shared delayed tooltip system. T6 supplies richer location content.
  game.on_hover(function(hover)
    tooltip.map_hover(hover, hover and {class = "location", args = {place = hover.id}} or nil)
  end)
end

-- Every update gives the pointer to the dispatcher (which makes scene calls only for a change a widget shows) and, when the window's
-- size changed, relayouts the anchored elements. An update with an unchanged size and nothing for the dispatcher makes no scene call.
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  listbar.update(update)
  if update.width ~= view.width or update.height ~= view.height then
    view.width = update.width
    view.height = update.height
    windows.update_viewport(view.width, view.height)
    for _, relayout in ipairs(relayouts) do
      relayout()
    end
    menu.relayout()
  end
  tooltip.update(update)
end)

print("ui: loaded")
log.info("loaded")


window.on_opened(function()
  log.info("the window is open")
end)

window.on_close_request(function(source)
  log.info("close requested by " .. source)
  -- Escape closes what is open and, with nothing left, opens the pause menu; the close button ends the run.
  return menu.close_request(source, view)
end)

function startup()
  print("ui: startup")
  log.info("startup")
end

function shutdown()
  print("ui: shutdown")
  log.info("shutdown")
end
