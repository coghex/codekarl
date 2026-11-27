-- The resize corner of a window: a 9 by 9 UI-pixel hit region at its bottom right that drags the window's size (shared by the event log and the
-- graph windows, CK-441). The window is kept at least `min_width` by `min_height` points and no larger than the room the view leaves from its
-- position.
--
--   resize_corner.new{window = window, view = view, prefix = "event_resize", min_width = 300 * P, min_height = 120 * P,
--     on_begin = function() end, on_change = function() end}
--
-- `min_width` and `min_height` are numbers or functions that give them, `view` is read live (`view.width`, `view.height`). `on_begin` runs when a
-- drag starts and `on_change` after each size it sets. The corner registers itself with the pointer and attaches itself to the window.
local ui = require("lib.ui")
local pointer = require("lib.pointer")

local P = ui.P
local resize_corner = {}

local function value_of(source)
  if type(source) == "function" then return source() end
  return source
end

function resize_corner.new(spec)
  local created = spec.window
  local corner = {group = ui.create_group(), key = ui.key(spec.prefix), dragging = false}
  corner.groups = {corner.group}
  function corner:layout()
    render.set_group_offset(self.group, created.x, created.y)
    render.set_hit(self.key, {{x = created.outer_width - 9 * P, y = created.outer_height - 9 * P, width = 9 * P, height = 9 * P}}, self.group)
  end
  function corner:on_event(event, eligible)
    if eligible == false then return end
    if event.kind == "down" then
      if spec.on_begin ~= nil then spec.on_begin() end
      self.dragging = {x = event.x, y = event.y, w = created.outer_width, h = created.outer_height}
      pointer.pressed(self.key, true)
    elseif self.dragging and (event.kind == "move" or event.kind == "up") then
      local min_width, min_height = value_of(spec.min_width), value_of(spec.min_height)
      local view = spec.view
      local w = math.max(min_width, math.min(self.dragging.w + event.x - self.dragging.x, math.max(min_width, view.width - created.x)))
      local h = math.max(min_height, math.min(self.dragging.h + event.y - self.dragging.y, math.max(min_height, view.height - created.y)))
      created:set_outer_size(ui.snap(w), ui.snap(h))
      if spec.on_change ~= nil then spec.on_change() end
      if event.kind == "up" then self.dragging = false; pointer.pressed(self.key, false) end
    elseif event.kind == "cancel" then self.dragging = false; pointer.pressed(self.key, false) end
  end
  function corner:functional() return true end
  function corner:on_hover() end
  function corner:on_overflow() self.dragging = false; pointer.pressed(self.key, false) end
  function corner:destroy()
    if self.closed then return end
    self.closed = true; pointer.unregister(self.key); render.destroy_group(self.group)
  end
  pointer.register(corner.key, corner, created)
  created:attach(corner)
  return corner
end

return resize_corner
