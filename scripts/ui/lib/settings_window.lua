-- The settings window (CK-301, slice A): live tooltips and map values plus a read-only view of input bindings.
local ui = require("lib.ui")
local button = require("lib.button")
local label = require("lib.label")
local text = require("lib.text")
local windows = require("lib.window")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local frame = require("lib.frame")
local scrollbar = require("lib.scrollbar")
local typing = require("lib.typing")

local P = ui.P
local WHITE = "ui/theme/white.png"
local SIDE_TABS = {"general", "map", "input"}
local WIDTH_TABS = {"general", "graphics", "map", "input"}
local LABEL_WIDTH = 84 * P
local SLIDER_WIDTH = 80 * P
local KEY_WIDTH = (font.layout("press a key").width + 8) * P
local KEY_HEIGHT = 13 * P
local KEY_GAP = 3 * P
local INPUT_FOOTER = 22 * P
local INPUT_GROUPS = {
  {"time", {{"time_pause", "pause"}, {"time_speed_1", "speed 1"}, {"time_speed_2", "speed 2"},
    {"time_speed_3", "speed 3"}, {"time_speed_4", "speed 4"}, {"time_speed_5", "speed 5"},
    {"time_faster", "faster"}, {"time_slower", "slower"}}},
  {"game", {{"game_quicksave", "quicksave"}, {"game_quickload", "quickload"}, {"close", "menu"}}},
  {"map", {{"map_pan_left", "pan left"}, {"map_pan_right", "pan right"}, {"map_pan_up", "pan up"},
    {"map_pan_down", "pan down"}, {"map_zoom_in", "zoom in"}, {"map_zoom_out", "zoom out"}}},
}

local settings_window = {}
local current
local cancel_wait

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

local function bindings_by_name()
  local by_name = {}
  for _, action in ipairs(settings.bindings()) do
    by_name[action.name] = {
      bindings = action.bindings or {}, fixed = action.fixed == true,
      default = action.default or {},
    }
  end
  return by_name
end

local function key_name(binding)
  local simple = {
    ArrowLeft = "Left", ArrowRight = "Right", ArrowUp = "Up", ArrowDown = "Down",
    BracketLeft = "[", BracketRight = "]", Equal = "=", Minus = "-",
    Semicolon = ";", Quote = "'", Backslash = "\\", Comma = ",", Period = ".",
    Slash = "/", Backquote = "`", Space = "Space", Escape = "Escape", Enter = "Enter", Tab = "Tab",
  }
  local parts = {}
  for part in binding:gmatch("[^+]+") do
    local digit = part:match("^Digit([0-9])$")
    local letter = part:match("^Key([A-Z])$")
    local shown = digit or letter or simple[part] or part
    if part == "Control" then shown = "Ctrl"
    elseif part == "Alt" then shown = "Option"
    elseif part == "Meta" then shown = "Cmd" end
    parts[#parts + 1] = shown
  end
  return table.concat(parts, "+")
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
  self.track = render.create_quad{texture = ui.texture(WHITE), x = self.x, y = self.y + 3 * P,
    width = SLIDER_WIDTH, height = 2 * P, group = page.group, tint = track_colour}
  self.fill = render.create_quad{texture = ui.texture(WHITE), x = self.x, y = self.y + 3 * P,
    width = P, height = 2 * P, group = page.group, tint = fill_colour}
  self.knob = render.create_quad{texture = ui.texture(WHITE), x = self.x, y = self.y,
    width = 3 * P, height = 7 * P, group = page.group, tint = fill_colour}
  local maximum_offset = SLIDER_WIDTH - 3 * P
  local function offset()
    local fraction = (self.value - self.minimum) / (self.maximum - self.minimum)
    return ui.snap(maximum_offset * fraction)
  end
  local function place()
    local dx = offset()
    render.set_position(self.knob, self.x + dx, self.y)
    render.set_rect(self.fill, self.x, self.y + 3 * P, math.max(P, dx + 2 * P), 2 * P)
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
  render.set_hit(self.key, {{x = self.x - 2 * P, y = self.y, width = SLIDER_WIDTH + 4 * P, height = 7 * P}}, page.group)
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
    rows_group = render.create_group(), scroll = 0, row_height = 0,
    row_view_height = state.content_height - INPUT_FOOTER,
  }
  page.bar = scrollbar.new{window = state.window, owner = {
    scroll = function() return page.scroll end,
    set_scroll = function(pos) page:set_scroll(pos) end,
    wheel = function(event) page:wheel(event) end,
  }}
  page.groups = {page.group, page.rows_group, page.bar.group}
  function page:set_scroll(pos)
    self.scroll = math.max(0, math.min(math.max(0, self.row_height - self.row_view_height), ui.snap(pos)))
    render.set_group_offset(self.rows_group, (self.origin_x or 0) + self.x, (self.origin_y or 0) - self.scroll)
    self.bar:update(self.row_view_height, self.row_height, self.scroll)
  end
  function page:wheel(event)
    if event.kind ~= "scroll" then return end
    local amount = event.amount or 0
    local step = event.unit == "lines" and 15 * P or 1
    self:set_scroll(self.scroll - amount * step)
  end
  page.area_key = ui.key("settings_input_area")
  page.area = {
    on_event = function(self, event)
      if event.kind == "scroll" then page:wheel(event) end
    end,
    on_hover = function() end,
    on_overflow = function() end,
    functional = function() return false end,
  }
  page.item = {
    groups = page.groups,
    layout = function(_, ox, oy, clip)
      page.x = state.content_x
      page.origin_x, page.origin_y = ox, oy
      render.set_group_offset(page.group, ox + page.x, oy)
      render.set_group_offset(page.rows_group, ox + page.x, oy - page.scroll)
      render.set_group_clip(page.group, {
        x = clip.x + page.x, y = clip.y, width = math.max(P, clip.width - page.x), height = clip.height,
      })
      render.set_group_clip(page.rows_group, {
        x = clip.x + page.x, y = clip.y, width = math.max(P, clip.width - page.x - scrollbar.RESERVE),
        height = page.row_view_height,
      })
      render.set_hit(page.area_key, page.input and {{x = page.x, y = 0, width = page.room, height = page.row_view_height}} or nil,
        state.window.content)
      page.bar:place(ox + page.x + page.room - scrollbar.WIDTH, oy, {
        x = clip.x + page.x, y = clip.y, width = page.room, height = page.row_view_height,
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
      page.bar:destroy()
      render.destroy_group(page.group)
      render.destroy_group(page.rows_group)
    end,
  }
  state.page = page
  state.window:attach(page.item)
  page.bar:update(page.row_view_height, 0, 0)
  return page
end

local function add_defaults(state, page, tab, y)
  local b = button.new{
    x = 0, y = y, text = "defaults", kind = "window", group = page.group, owner = page.owner,
    on_click = function()
      local setting = tab == "general" and "tooltips" or tab == "bindings" and "bindings" or "map"
      settings.reset(setting)
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
  label_at(page.group, 0, 16 * P, "show delay", enabled_colour, LABEL_WIDTH)
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

local ACTION_LABEL = {
  close = "the menu", time_pause = "pause", time_speed_1 = "speed 1", time_speed_2 = "speed 2",
  time_speed_3 = "speed 3", time_speed_4 = "speed 4", time_speed_5 = "speed 5", time_faster = "faster",
  time_slower = "slower", game_quicksave = "quicksave", game_quickload = "quickload",
  map_pan_left = "pan left", map_pan_right = "pan right", map_pan_up = "pan up", map_pan_down = "pan down",
  map_zoom_in = "zoom in", map_zoom_out = "zoom out",
}

local function copy_keys(keys)
  local copy = {}
  for index, key in ipairs(keys or {}) do copy[index] = key end
  return copy
end

local function compact(keys)
  local out = {}
  for _, key in ipairs(keys) do if key ~= nil and key ~= "" then out[#out + 1] = key end end
  return out
end

local function set_slot(keys, index, value)
  if index == 2 and keys[1] == nil then keys[1] = value
  else keys[index] = value end
end

local function set_escape_hint(state, waiting)
  local hint = state.controls.escape_cancel
  if hint ~= nil then hint:set_text(waiting and "Escape cancels" or "") end
end

cancel_wait = function(state)
  if state.waiting == nil then return end
  pcall(settings.cancel_capture)
  local slot = state.waiting.slot
  state.waiting = nil
  set_escape_hint(state, false)
  if slot ~= nil and slot.refresh ~= nil then slot:refresh() end
end

local function apply_bindings(state, changes)
  local ok, failure = pcall(settings.set_bindings, changes)
  if not ok then
    log.info("settings.set_bindings refused: " .. tostring(failure))
    return false
  end
  if current == state and not state.window.closed then state:show("input") end
  return true
end

local function slot_list(actions, action)
  local item = actions[action]
  return item and copy_keys(item.bindings) or {}
end

local function change_slot(state, action, index, value)
  local actions = bindings_by_name()
  local keys = slot_list(actions, action)
  set_slot(keys, index, value)
  return apply_bindings(state, {[action] = compact(keys)})
end

local function clear_slot(state, action, index)
  local actions = bindings_by_name()
  local keys = slot_list(actions, action)
  if keys[index] == nil then return false end
  table.remove(keys, index)
  return apply_bindings(state, {[action] = keys})
end

local function ordered_player_actions(actions)
  local out, seen = {}, {}
  for _, group in ipairs(INPUT_GROUPS) do
    for _, row in ipairs(group[2]) do
      if actions[row[1]] ~= nil then
        out[#out + 1] = row[1]
        seen[row[1]] = true
      end
    end
  end
  if actions.close ~= nil and not seen.close then out[#out + 1] = "close" end
  for name in pairs(actions) do
    if not seen[name] and name ~= "close" then
      -- The host exposes only the player's actions; keep an unexpected addition visible and deterministic.
      out[#out + 1] = name
    end
  end
  return out
end

local function named_owners(owners)
  local labels = {}
  for _, action in ipairs(owners) do labels[#labels + 1] = ACTION_LABEL[action] or action end
  if #labels == 1 then return labels[1] end
  local last = table.remove(labels)
  return table.concat(labels, ", ") .. " and " .. last
end

local function possessive_owners(owners)
  local labels = {}
  for _, action in ipairs(owners) do labels[#labels + 1] = (ACTION_LABEL[action] or action) .. "'s" end
  if #labels == 1 then return labels[1] end
  local last = table.remove(labels)
  return table.concat(labels, ", ") .. " and " .. last
end

local function remove_key(keys, key)
  local out = {}
  for _, candidate in ipairs(keys) do if candidate ~= key then out[#out + 1] = candidate end end
  return out
end

local function modal_backdrop_widget()
  return {
    on_event = function() end,
    on_hover = function() end,
    on_overflow = function() end,
    functional = function() return false end,
  }
end

local function restore_modal_layer(prompt)
  if prompt.window ~= nil then windows.clear_modal_overlay(prompt.window, prompt.backdrop_group) end
  pointer.unregister(prompt.backdrop_key)
  render.set_hit(prompt.backdrop_key, nil, prompt.backdrop_group)
  render.destroy_group(prompt.backdrop_group)
  if current == prompt.state and prompt.state.prompt == prompt then prompt.state.prompt = nil end
end

local function resolve_conflict(state, decision)
  local prompt = state.prompt
  if prompt == nil then return end
  local target = prompt.target
  local changes = {}
  local target_keys = slot_list(bindings_by_name(), target.action)
  if decision == "clear" then
    if target_keys[target.index] ~= nil then
      table.remove(target_keys, target.index)
      changes[target.action] = target_keys
    end
  elseif decision == "replace" or decision == "swap" then
    set_slot(target_keys, target.index, target.key)
    changes[target.action] = compact(target_keys)
    local latest = bindings_by_name()
    for _, owner in ipairs(prompt.owners) do
      local keys = slot_list(latest, owner)
      if decision == "swap" and #prompt.owners == 1 and owner == prompt.owners[1] then
        for index, key in ipairs(keys) do
          if key == target.key then keys[index] = target.old_key end
        end
      else
        keys = remove_key(keys, target.key)
      end
      changes[owner] = compact(keys)
    end
  end
  if next(changes) ~= nil then apply_bindings(state, changes) end
  if state.prompt == prompt then prompt.window:close() end
end

local function show_prompt(state, target, owners, actions, immutable, can_swap)
  local key, action = key_name(target.key), ACTION_LABEL[target.action] or target.action
  local owner_text = named_owners(owners)
  local owners_possessive = possessive_owners(owners)
  local lines, choices
  if immutable then
    lines = {key .. " is " .. owners_possessive .. " key.", "It cannot be changed here."}
    choices = {"cancel"}
  elseif #owners > 1 then
    lines = {key .. " is " .. owners_possessive .. " key.", "Use it for " .. action .. " instead?", "Their slots will be empty."}
    choices = {"replace", "clear", "cancel"}
  elseif target.old_key == nil or not can_swap then
    lines = {key .. " is " .. owners_possessive .. " key.", "Use it for " .. action .. " instead?", owner_text .. "'s slot will be empty."}
    choices = {"replace", "clear", "cancel"}
  else
    lines = {key .. " is " .. owners_possessive .. " key.", "Use it for " .. action .. " instead?",
      "swap: " .. owner_text .. " gets " .. key_name(target.old_key) .. ".",
      "replace: " .. owner_text .. "'s slot will be empty."}
    choices = {"swap", "replace", "clear", "cancel"}
  end
  local line_width = 0
  for _, line in ipairs(lines) do line_width = math.max(line_width, text_width(line)) end
  local buttons_width = 0
  for index, choice in ipairs(choices) do
    buttons_width = buttons_width + button.measure(choice)
    if index > 1 then buttons_width = buttons_width + KEY_GAP end
  end
  local width = math.max(line_width + 10 * P, buttons_width + 8 * P)
  local _, button_height = button.measure("cancel")
  local height = (#lines * 15 + 7) * P + button_height
  local prompt = {
    state = state, target = target, owners = owners, window = nil,
    backdrop_group = ui.create_group(), view = state.view,
  }
  prompt.backdrop = render.create_quad{
    texture = ui.texture(WHITE), x = 0, y = 0, width = state.view.width, height = state.view.height,
    group = prompt.backdrop_group, tint = theme.colour("body"):sub(1, 7) .. "80",
  }
  prompt.backdrop_key = ui.key("key_prompt_backdrop")
  render.set_hit(prompt.backdrop_key, {{x = 0, y = 0, width = state.view.width, height = state.view.height}}, prompt.backdrop_group)
  pointer.register(prompt.backdrop_key, modal_backdrop_widget())
  prompt.window = windows.new{
    title = "Key in use", kind = "window", fixed = true, draggable = false, modal = true,
    content_width = width, content_height = height,
    on_close = function() restore_modal_layer(prompt) end,
  }
  windows.set_modal_overlay(prompt.window, prompt.backdrop_group)
  local y = 0
  for _, line in ipairs(lines) do
    if line ~= "" then label_at(prompt.window.content, 0, y, line, theme.colour("text"), width) end
    y = y + 15 * P
  end
  local _, button_height = button.measure("cancel")
  prompt.buttons = button.flow{
    x = width - buttons_width, y = height - button_height, room = buttons_width,
    items = choices, group = prompt.window.content, owner = prompt.window, kind = "window",
    on_choose = function(choice)
      if choice ~= "cancel" then resolve_conflict(state, choice)
      elseif state.prompt == prompt then prompt.window:close() end
    end,
  }
  state.prompt = prompt
  local outer_width, outer_height = prompt.window:size()
  local x, py = ui.anchor("centre", outer_width, outer_height, 0, 0, state.view.width, state.view.height)
  prompt.window:move_to(x, py)
end

local function captured_key(state, waiting, key)
  local actions = bindings_by_name()
  local own = slot_list(actions, waiting.action)
  if own[1] == key or own[2] == key then return end
  local owners = {}
  for _, action in ipairs(ordered_player_actions(actions)) do
    if action ~= waiting.action then
      for _, bound in ipairs(slot_list(actions, action)) do
        if bound == key then owners[#owners + 1] = action; break end
      end
    end
  end
  if #owners == 0 then
    change_slot(state, waiting.action, waiting.index, key)
    return
  end
  local immutable = false
  for _, owner in ipairs(owners) do
    local item = actions[owner]
    if owner == "close" or item.fixed or #item.bindings > 2 then immutable = true end
  end
  local old_key = own[waiting.index]
  local owner_holds_old = false
  if #owners == 1 and old_key ~= nil then
    for _, bound in ipairs(slot_list(actions, owners[1])) do
      if bound == old_key then owner_holds_old = true end
    end
  end
  show_prompt(state, {
    action = waiting.action, index = waiting.index, key = key, old_key = old_key,
  }, owners, actions, immutable, old_key ~= nil and not owner_holds_old)
end

local function make_slot(state, page, action, index, key, y, read_only)
  local slot = {
    key = ui.key("settings_key_slot"), action = action, index = index, value = key,
    x = 70 * P + (index - 1) * (KEY_WIDTH + KEY_GAP), y = y,
    read_only = read_only, group = page.rows_group,
  }
  slot.frame = frame.new{
    x = slot.x, y = slot.y, width = KEY_WIDTH, height = KEY_HEIGHT, cut = 3, corners = "tl_br",
    fill = theme.colour("body"), line = read_only and theme.colour("track") or theme.kind("window").outline,
    group = page.rows_group,
  }
  slot.line = text.line(slot.x, slot.y, page.rows_group, read_only and theme.colour("dim") or theme.colour("text"))
  function slot:refresh()
    local waiting = state.waiting ~= nil and state.waiting.slot == self
    local shown = waiting and "press a key" or self.value and key_name(self.value) or "-"
    local room = KEY_WIDTH - 8 * P
    shown = fit(shown, room)
    text.set(self.line, shown)
    text.move(self.line, self.x + (KEY_WIDTH - self.line.width * P) / 2,
      self.y + (KEY_HEIGHT - self.line.height * P) / 2)
    local fill = waiting and theme.kind("window").hot or theme.colour("body")
    local outline = read_only and theme.colour("track") or waiting and theme.kind("window").hot or theme.kind("window").outline
    slot.frame:set_colours(fill, outline)
    text.set_colour(self.line, read_only and theme.colour("dim") or waiting and theme.kind("window").pressed_text
      or self.value == nil and theme.colour("dim") or theme.colour("text"))
  end
  function slot:on_event(event, eligible)
    if read_only or eligible == false then return end
    if event.kind == "button_down" and event.button == "right" then
      cancel_wait(state)
      clear_slot(state, action, index)
    elseif event.kind == "up" and event.inside == true then
      if state.waiting ~= nil then cancel_wait(state) end
      local request = ui.key("settings_capture")
      state.waiting = {action = action, index = index, slot = self, id = request}
      typing.capture(request, settings.capture_key)
      self:refresh()
      set_escape_hint(state, true)
    end
  end
  function slot:on_hover() end
  function slot:on_overflow() end
  function slot:functional() return not read_only end
  render.set_hit(slot.key, {{x = slot.x, y = slot.y, width = KEY_WIDTH, height = KEY_HEIGHT}}, page.rows_group)
  pointer.register(slot.key, slot, page.owner)
  slot:refresh()
  return slot
end

local function make_action_row(state, page, action, shown, keys, y, fixed)
  local read_only = fixed or #keys > 2
  label_at(page.rows_group, 4 * P, y + P, shown, read_only and theme.colour("dim") or theme.colour("text"), 62 * P)
  local slots = {}
  for index = 1, 2 do slots[index] = make_slot(state, page, action, index, keys[index], y, read_only) end
  if #keys > 2 then
    label_at(page.rows_group, 70 * P + 2 * KEY_WIDTH + KEY_GAP, y + P, "+" .. tostring(#keys - 2), theme.colour("dim"), 26 * P)
  end
  state.controls.key_slots[action] = slots
  state.controls.rows[action] = {read_only = read_only, slots = slots}
end

local function show_input(state, page)
  local actions = bindings_by_name()
  page.input = true
  pointer.register(page.area_key, page.area, page.owner)
  state.controls.key_slots, state.controls.rows = {}, {}
  page.row_view_height = math.max(P, state.content_height - INPUT_FOOTER)
  local y = 0
  for _, group in ipairs(INPUT_GROUPS) do
    local present = false
    for _, row in ipairs(group[2]) do if actions[row[1]] ~= nil then present = true end end
    if present then
      label_at(page.rows_group, 4 * P, y + P, group[1], theme.colour("dim"), page.room - 8 * P)
      y = y + 15 * P
      for _, row in ipairs(group[2]) do
        local action = actions[row[1]]
        if action ~= nil then
          make_action_row(state, page, row[1], row[2], action.bindings, y, action.fixed or row[1] == "close")
          y = y + 15 * P
        end
      end
    end
  end
  page.row_height = y
  page:set_scroll(0)
  page.bar:update(page.row_view_height, page.row_height, page.scroll)
  local footer_y = state.content_height - 18 * P
  add_defaults(state, page, "bindings", footer_y)
  state.controls.escape_cancel = label.new{
    x = state.controls.defaults.width + 5 * P, y = footer_y + 2 * P, text = "",
    group = page.group, colour = theme.colour("dim"),
  }
  state.page = page
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
      if state.prompt ~= nil then state.prompt.window:close() end
      if state.waiting ~= nil then
        pcall(settings.cancel_capture)
        state.waiting = nil
      end
      clear_page(state)
      if current == state then current = nil end
    end,
  }
  function state:show(name)
    if self.window.closed then return end
    cancel_wait(self)
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
    local prompt = self.prompt
    if prompt ~= nil and not prompt.window.closed then
      prompt.view = self.view
      render.set_rect(prompt.backdrop, 0, 0, self.view.width, self.view.height)
      render.set_hit(prompt.backdrop_key, {{x = 0, y = 0, width = self.view.width, height = self.view.height}}, prompt.backdrop_group)
      local width, height = prompt.window:size()
      local x, y = ui.anchor("centre", width, height, 0, 0, self.view.width, self.view.height)
      prompt.window:move_to(x, y)
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

function settings_window.update(update)
  local state = current
  if state == nil or state.window.closed then return end
  if state.waiting ~= nil and update ~= nil and update.pointer ~= nil then
    for _, event in ipairs(update.pointer.events or {}) do
      if event.kind == "down" and event.key ~= state.waiting.slot.key then
        cancel_wait(state)
        break
      end
    end
  end
  local capture = update ~= nil and update.capture or nil
  local waiting = state.waiting
  if capture == nil or waiting == nil or capture.id ~= waiting.id then return end
  state.waiting = nil
  set_escape_hint(state, false)
  waiting.slot:refresh()
  if capture.kind == "binding" then
    if capture.value == "Escape" then return
    elseif capture.value == "Backspace" then clear_slot(state, waiting.action, waiting.index)
    else captured_key(state, waiting, capture.value) end
  end
end

function settings_window.close()
  if current ~= nil and not current.window.closed then
    if current.prompt ~= nil then current.prompt.window:close() end
    cancel_wait(current)
    current.window:close()
  end
end

return settings_window
