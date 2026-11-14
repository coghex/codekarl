-- The settings window (CK-301, slice A): live tooltips and map values plus a read-only view of input bindings.
local ui = require("lib.ui")
local button = require("lib.button")
local label = require("lib.label")
local text = require("lib.text")
local windows = require("lib.window")
local theme = require("lib.theme")
local pointer = require("lib.pointer")

local P = ui.P
local WHITE = "ui/theme/white.png"
local SIDE_TABS = {"general", "map", "input"}
local WIDTH_TABS = {"general", "graphics", "map", "input"}
local LABEL_WIDTH = 84 * P
local SLIDER_WIDTH = 80 * P
local GAP = 3 * P

local settings_window = {}
local current

local function text_width(value)
  return font.layout(value).width * P
end

local function fit(value, width)
  if text_width(value) <= width then return value end
  local count = utf8.len(value) or #value
  while count > 0 do
    local candidate = value:sub(1, utf8.offset(value, count + 1) - 1) .. ".."
    if text_width(candidate) <= width then return candidate end
    count = count - 1
  end
  return ""
end

local function label_at(group, x, y, value, colour, room)
  local line = label.new{x = x, y = y, text = fit(value, room), group = group, colour = colour}
  return line
end

local function key_label(name)
  local digits = name:match("^Digit([0-9])$")
  if digits then return digits end
  local simple = {
    BracketLeft = "[", BracketRight = "]", Equal = "=", Minus = "-",
  }
  return simple[name] or name
end

local function actions_by_name()
  local by_name = {}
  for _, action in ipairs(settings.bindings()) do
    by_name[action.name] = action.bindings or {}
  end
  return by_name
end

local function display_bindings(actions)
  local out = {}
  local arrows = {
    {"map_pan_left", "ArrowLeft"}, {"map_pan_right", "ArrowRight"},
    {"map_pan_up", "ArrowUp"}, {"map_pan_down", "ArrowDown"},
  }
  local arrow_set = true
  for _, pair in ipairs(arrows) do
    local keys = actions[pair[1]] or {}
    if #keys ~= 1 or keys[1] ~= pair[2] then arrow_set = false end
  end
  local rows = {
    {"pause", {"time_pause"}},
    {"speed 1 to 5", {"time_speed_1", "time_speed_2", "time_speed_3", "time_speed_4", "time_speed_5"}},
    {"faster, slower", {"time_faster", "time_slower"}},
    {"quicksave", {"game_quicksave"}},
    {"quickload", {"game_quickload"}},
    {"move the map", {"map_pan_left", "map_pan_right", "map_pan_up", "map_pan_down"}},
    {"zoom in, out", {"map_zoom_in", "map_zoom_out"}},
    {"close, menu", {"close"}},
  }
  for _, row in ipairs(rows) do
    local shown = {}
    if row[1] == "move the map" and arrow_set then
      shown[1] = "arrows"
    else
      for _, name in ipairs(row[2]) do
        for _, key in ipairs(actions[name] or {}) do
          if key:find("+", 1, true) then shown[#shown + 1] = key
          else shown[#shown + 1] = key_label(key) end
        end
      end
    end
    out[#out + 1] = {row[1], table.concat(shown, "  ")}
  end
  return out
end

local function slider(state, page, name, key, y, minimum, maximum, step, value, shown, enabled, colour)
  local self = {
    key = ui.key("settings_slider"), owner = page.owner, group = page.group, window = state.window,
    name = name, setting = key, x = LABEL_WIDTH, y = y + 4 * P,
    minimum = minimum, maximum = maximum, step = step, value = value,
    shown = shown, enabled = enabled ~= false, dragging = false, colour = colour,
  }
  local track_colour = self.enabled and theme.colour("track") or theme.colour("dim")
  local fill_colour = self.enabled and theme.colour("text") or theme.colour("dim")
  self.track = render.create_quad{texture = ui.texture(WHITE), x = self.x, y = self.y + 5 * P,
    width = SLIDER_WIDTH, height = 2 * P, group = page.group, tint = track_colour}
  self.fill = render.create_quad{texture = ui.texture(WHITE), x = self.x, y = self.y + 5 * P,
    width = P, height = 2 * P, group = page.group, tint = fill_colour}
  self.knob = render.create_quad{texture = ui.texture(WHITE), x = self.x, y = self.y + 3 * P,
    width = 3 * P, height = 7 * P, group = page.group, tint = fill_colour}
  local maximum_offset = SLIDER_WIDTH - 3 * P
  local function offset()
    local fraction = (self.value - self.minimum) / (self.maximum - self.minimum)
    return ui.snap(maximum_offset * fraction)
  end
  local function place()
    local dx = offset()
    render.set_position(self.knob, self.x + dx, self.y + 3 * P)
    render.set_rect(self.fill, self.x, self.y + 5 * P, math.max(P, dx + 2 * P), 2 * P)
    local wx = select(1, self.window:content_origin())
    local wy = select(2, self.window:content_origin())
    self.abs_x = wx + page.x + self.x
    self.abs_y = wy + self.y
  end
  page.sliders = page.sliders or {}
  page.sliders[#page.sliders + 1] = self
  local value_room = math.max(P, page.room - LABEL_WIDTH - 86 * P)
  local function formatted(next_value)
    if name == "show delay" or name == "lock timer" then return string.format("%.1f s", next_value) end
    return tostring(next_value) .. (name == "scroll step" and " points" or "")
  end
  self.value_label = label_at(page.group, LABEL_WIDTH + 86 * P, y + 2 * P, shown or formatted(value), colour, value_room)
  function self:set(next_value)
    local count = math.floor((next_value - self.minimum) / self.step + 0.5)
    local snapped = self.minimum + count * self.step
    snapped = math.max(self.minimum, math.min(self.maximum, snapped))
    if snapped == self.value then return end
    self.value = snapped
    settings.set(self.setting, snapped)
    -- Keep the captured widget alive across frame updates; only the slider's own visuals change during a drag.
    place()
    self.value_label:set_text(fit(formatted(snapped), value_room))
  end
  function self:on_event(event, eligible)
    if not self.enabled or eligible == false then return end
    if event.kind == "down" then
      self.dragging = true
      pointer.pressed(self.key, true)
      self:set(self.minimum + math.max(0, math.min(1,
        (event.x - self.abs_x) / maximum_offset)) * (self.maximum - self.minimum))
    elseif event.kind == "move" and self.dragging then
      self:set(self.minimum + math.max(0, math.min(1,
        (event.x - self.abs_x) / maximum_offset)) * (self.maximum - self.minimum))
    elseif event.kind == "up" and self.dragging then
      self:set(self.minimum + math.max(0, math.min(1,
        (event.x - self.abs_x) / maximum_offset)) * (self.maximum - self.minimum))
      self.dragging = false
      pointer.pressed(self.key, false)
    elseif event.kind == "cancel" then
      self.dragging = false
      pointer.pressed(self.key, false)
    end
  end
  function self:on_hover() end
  function self:on_overflow()
    self.dragging = false
    pointer.pressed(self.key, false)
  end
  function self:functional() return self.enabled end
  function self:destroy() pointer.unregister(self.key) end
  render.set_hit(self.key, {{x = self.x - 2 * P, y = self.y, width = SLIDER_WIDTH + 4 * P, height = 10 * P}}, page.group)
  pointer.register(self.key, self, page.owner)
  place()
  state.controls[name] = self
  return self
end

local function clear_page(state)
  if state.page ~= nil then
    if not state.page.destroyed then
      state.window:detach(state.page.item)
      state.page.item:destroy()
    end
    state.page = nil
  end
  state.controls = {}
end

local function make_page(state)
  local page = {
    group = render.create_group(), owner = {}, x = state.content_x,
    room = state.content_width - state.content_x,
  }
  page.item = {
    groups = {page.group},
    layout = function(_, ox, oy, clip)
      page.x = state.content_x
      render.set_group_offset(page.group, ox + page.x, oy)
      render.set_group_clip(page.group, {
        x = clip.x + page.x, y = clip.y, width = math.max(P, clip.width - page.x), height = clip.height,
      })
      for _, control in ipairs(page.sliders or {}) do
        local wx, wy = state.window:content_origin()
        control.abs_x = wx + page.x + control.x
        control.abs_y = wy + control.y
      end
    end,
    destroy = function()
      if page.destroyed then return end
      page.destroyed = true
      pointer.unregister_owner(page.owner)
      render.destroy_group(page.group)
    end,
  }
  state.page = page
  state.window:attach(page.item)
  return page
end

local function add_defaults(state, page, tab, y)
  local b = button.new{
    x = 0, y = y, text = "defaults", kind = "window", group = page.group, owner = page.owner,
    on_click = function()
      settings.reset(tab == "general" and "tooltips" or "map")
      pointer.defer(function()
        if current == state and not state.window.closed then state:show(tab) end
      end)
    end,
  }
  state.controls.defaults = b
end

local function show_general(state, page)
  local values = settings.tooltips()
  label_at(page.group, 0, 0, "tooltips", theme.colour("dim"), page.room)
  local enabled_colour = theme.colour("text")
  local dim_colour = theme.colour("dim")
  slider(state, page, "show delay", "ui.tooltips.delay", 14 * P, 0, 2, 0.1, values.delay,
    string.format("%.1f s", values.delay), true, enabled_colour)
  label_at(page.group, 0, 36 * P, "lock", enabled_colour, LABEL_WIDTH)
  local choices, height = button.flow{
    x = LABEL_WIDTH, y = 34 * P, room = page.room - LABEL_WIDTH,
    items = {"disabled", "middle mouse", "timer"},
    selected = values.lock == "middle_mouse" and "middle mouse" or values.lock,
    group = page.group, owner = page.owner, kind = "window",
    on_choose = function(label)
      local choice = label == "middle mouse" and "middle_mouse" or label
      settings.set("ui.tooltips.lock", choice)
      pointer.defer(function()
        if current == state and not state.window.closed then state:show("general") end
      end)
    end,
  }
  state.controls.lock = choices
  local y = 34 * P + height + 5 * P
  local active = values.lock == "timer"
  label_at(page.group, 0, y + 2 * P, "lock timer", active and enabled_colour or dim_colour, LABEL_WIDTH)
  slider(state, page, "lock timer", "ui.tooltips.lock_timer", y, 0.3, 3, 0.1,
    values.lock_timer, string.format("%.1f s", values.lock_timer), active, active and enabled_colour or dim_colour)
  add_defaults(state, page, "general", state.content_height - 18 * P)
end

local function show_map(state, page)
  local values = settings.map()
  local rows = {
    {"pan speed", "map.pan_pixels_per_frame", 1, 32, values.pan_pixels_per_frame, ""},
    {"zoom speed", "map.zoom_steps_per_second", 1, 60, values.zoom_steps_per_second, ""},
    {"scroll step", "map.scroll_points_per_step", 1, 256, values.scroll_points_per_step, " points"},
  }
  for index, row in ipairs(rows) do
    local y = (index - 1) * 20 * P
    label_at(page.group, 0, y + 2 * P, row[1], theme.colour("text"), LABEL_WIDTH)
    slider(state, page, row[1], row[2], y, row[3], row[4], 1, row[5], tostring(row[5]) .. row[6], true, theme.colour("text"))
  end
  add_defaults(state, page, "map", state.content_height - 18 * P)
end

local function show_input(state, page)
  local rows = display_bindings(actions_by_name())
  for index, row in ipairs(rows) do
    local y = (index - 1) * 15 * P
    label_at(page.group, 0, y + P, row[1], theme.colour("text"), LABEL_WIDTH - P)
    label_at(page.group, LABEL_WIDTH, y + P, row[2], theme.colour("text"), page.room - LABEL_WIDTH)
  end
  label_at(page.group, 0, #rows * 15 * P + 4 * P, "changing keys comes later", theme.colour("dim"), page.room)
end

local function make_tabs(state)
  local colours = theme.kind("window")
  state.tabs = {}
  for index, name in ipairs(SIDE_TABS) do
    local tab = button.new{
      x = 0, y = (index - 1) * 13 * P, width = state.tab_width - 3 * P,
      text = name, kind = "window", group = state.window.content, owner = state.window,
      on_click = function() state:show(name) end,
    }
    tab.height = 13 * P
    tab.text_x = 3 * P
    tab.frame:set_rect(tab.x, tab.y, tab.width, tab.height)
    text.move(tab.line, tab.x + tab.text_x, tab.y + P)
    render.set_hit(tab.key, {{x = tab.x, y = tab.y, width = tab.width, height = tab.height}}, state.window.content)
    tab:set_selected(name == state.selected)
    state.tabs[name] = tab
  end
  state.divider = render.create_quad{
    texture = ui.texture(WHITE), x = state.tab_width, y = 0, width = P,
    height = state.content_height - 6 * P, group = state.window.content, tint = colours.outline,
  }
end

function settings_window.current()
  return current
end

function settings_window.open(view)
  if current ~= nil and not current.window.closed then
    current.window:raise()
    return current.window
  end
  local tab_width = 0
  for _, name in ipairs(WIDTH_TABS) do tab_width = math.max(tab_width, font.layout(name).width) end
  tab_width = (tab_width + 12) * P
  local state = {
    view = view, tab_width = tab_width, content_width = 272 * P + tab_width - 40 * P - 6 * P,
    content_height = 168 * P, selected = "general", controls = {}, tabs = {}, tab_order = SIDE_TABS,
  }
  state.content_x = tab_width + 6 * P
  state.window = windows.new{
    title = "Settings", kind = "window", draggable = false,
    content_width = state.content_width, content_height = state.content_height,
    modal = true,
    on_close = function()
      clear_page(state)
      if current == state then current = nil end
    end,
  }
  function state:show(name)
    if self.window.closed then return end
    self.selected = name
    for tab_name, tab in pairs(self.tabs) do tab:set_selected(tab_name == name) end
    clear_page(self)
    local page = make_page(self)
    if name == "general" then show_general(self, page)
    elseif name == "map" then show_map(self, page)
    else show_input(self, page) end
  end
  function state:relayout(next_view)
    if next_view ~= nil then self.view = next_view end
    if not self.window.closed then
      local width, height = self.window:size()
      local x, y = ui.anchor("centre", width, height, 0, 0, self.view.width, self.view.height)
      self.window:move_to(x, y)
    end
  end
  make_tabs(state)
  local x, y = ui.anchor("centre", select(1, state.window:size()), select(2, state.window:size()),
    0, 0, view.width, view.height)
  state.window:move_to(x, y)
  current = state
  state:show("general")
  return state.window
end

function settings_window.relayout(view)
  if current ~= nil then current:relayout(view) end
end

function settings_window.close()
  if current ~= nil and not current.window.closed then current.window:close() end
end

return settings_window
