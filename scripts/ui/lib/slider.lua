-- Shared stepped slider used by settings and the event-log picker.
local ui = require("lib.ui")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local P = ui.P
local WHITE = "ui/theme/white.png"
local LABEL_WIDTH = 84 * P
local SLIDER_WIDTH = 80 * P
local label = require("lib.label")
local slider = {}
local function fit(value, width)
  if font.layout(value).width * P <= width then return value end
  local count = utf8.len(value) or #value
  while count > 0 do
    local candidate = value:sub(1, utf8.offset(value, count + 1) - 1) .. ".."
    if font.layout(candidate).width * P <= width then return candidate end
    count = count - 1
  end
  return ""
end
local function label_at(group, x, y, value, colour, room)
  return label.new{x = x, y = y, text = fit(value, room), group = group, colour = colour}
end
function slider.new(state, page, name, key, y, minimum, maximum, step, value, shown, enabled, colour, options)
  options = options or {}
  local label_width = options.x or LABEL_WIDTH
  local self = {
    key = ui.key("settings_slider"), owner = page.owner, group = page.group, window = state.window,
    name = name, setting = key, x = label_width, y = y + 4 * P,
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
  end
  page.sliders = page.sliders or {}
  page.sliders[#page.sliders + 1] = self
  local value_room = math.max(P, page.room - label_width - 86 * P)
  local function formatted(next_value)
    if options.format then return options.format(next_value) end
    if name == "show delay" or name == "lock timer" then return string.format("%.1f s", next_value) end
    return tostring(next_value) .. (name == "scroll step" and " points" or "")
  end
  self.value_label = label_at(page.group, label_width + 86 * P, y + 2 * P, shown or formatted(value), colour, value_room)
  function self:set(next_value)
    local count = math.floor((next_value - self.minimum) / self.step + 0.5)
    local snapped = self.minimum + count * self.step
    snapped = math.max(self.minimum, math.min(self.maximum, snapped))
    if snapped == self.value then return end
    self.value = snapped
    if options.on_change then options.on_change(snapped) else settings.set(self.setting, snapped) end
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
  function self:place_abs(x, y)
    self.abs_x, self.abs_y = x + self.x, y + self.y
  end
  function self:set_enabled(enabled)
    self.enabled = enabled
    local track = enabled and theme.colour("track") or theme.colour("dim")
    local fill = enabled and theme.colour("text") or theme.colour("dim")
    render.set_tint(self.track, track)
    render.set_tint(self.fill, fill)
    render.set_tint(self.knob, fill)
    self.value_label:set_colour(enabled and self.colour or theme.colour("dim"))
    if enabled then
      render.set_hit(self.key, {{x = self.x - 2 * P, y = self.y, width = SLIDER_WIDTH + 4 * P, height = 7 * P}}, self.group)
    else
      render.set_hit(self.key, nil, self.group)
    end
  end
  function self:on_hover() end
  function self:on_overflow()
    self.dragging = false
    pointer.pressed(self.key, false)
  end
  function self:functional() return self.enabled end
  function self:destroy() pointer.unregister(self.key) end
  if self.enabled or not options.no_disabled_hit then render.set_hit(self.key, {{x = self.x - 2 * P, y = self.y, width = SLIDER_WIDTH + 4 * P, height = 7 * P}}, page.group) end
  pointer.register(self.key, self, page.owner)
  place()
  local wx, wy = self.window:content_origin()
  self:place_abs(wx + page.x, wy)
  state.controls[name] = self
  return self
end


return slider
