-- The expedition layer: map-projected chips and the selected trip's paths, below the UI.
local ui = require("lib.ui")
local pointer = require("lib.pointer")
local P = ui.P
local M = {}

-- Clip before rasterising, so a long off-screen route does not create off-screen quads.
local function clip(x1, y1, x2, y2, w, h)
  local dx, dy, lo, hi = x2 - x1, y2 - y1, 0, 1
  for _, bound in ipairs({{-dx, x1}, {dx, w - x1}, {-dy, y1}, {dy, h - y1}}) do
    local p, q = bound[1], bound[2]
    if p == 0 then if q < 0 then return nil end
    else
      local r = q / p
      if p < 0 then lo = math.max(lo, r) else hi = math.min(hi, r) end
      if lo > hi then return nil end
    end
  end
  return x1 + lo * dx, y1 + lo * dy, x1 + hi * dx, y1 + hi * dy
end

function M.new()
  local self = {paths = ui.create_group(), chips = ui.create_group(), quads = {}, hits = {}, selected = nil, signature = nil, world = nil, membership = nil}
  local function clear_drawn()
    for _, quad in ipairs(self.quads) do render.destroy_quad(quad) end
    self.quads = {}
  end
  local function remove_hit(id)
    local key = self.hits[id]
    if key then render.set_hit(key, nil, self.chips); pointer.unregister(key); self.hits[id] = nil end
  end
  local function quad(texture, x, y, w, h, group, tint)
    local q = render.create_quad{texture = ui.texture(texture), x = x, y = y, width = w, height = h, group = group, tint = tint}
    self.quads[#self.quads + 1] = q
  end
  local function line(a, b, colour, width, height)
    local x1, y1, x2, y2 = clip(a[1], a[2], b[1], b[2], width, height)
    if not x1 then return end
    x1, y1, x2, y2 = math.floor(x1 / P + .5), math.floor(y1 / P + .5), math.floor(x2 / P + .5), math.floor(y2 / P + .5)
    local dx, dy = math.abs(x2 - x1), math.abs(y2 - y1)
    local sx, sy = x1 < x2 and 1 or -1, y1 < y2 and 1 or -1
    local err = dx - dy
    while true do
      quad("ui/theme/white.png", x1 * P, y1 * P, 2 * P, 2 * P, self.paths, colour)
      if x1 == x2 and y1 == y2 then break end
      local twice = 2 * err
      if twice > -dy then err = err - dy; x1 = x1 + sx end
      if twice < dx then err = err + dx; y1 = y1 + sy end
    end
  end
  function self:clear()
    if self.selected ~= nil then self.selected = nil; self.signature = nil end
  end
  pointer.observe_map_click(function() self:clear() end)
  function self:update(data, width, height)
    if data == nil then return end
    if self.world ~= data.world then
      for id in pairs(self.hits) do remove_hit(id) end
      self.world = data.world; self:clear(); self.signature = nil; self.membership = nil
    end
    if not data.shown then self:clear() end
    local chosen = nil
    for _, trip in ipairs(data.trips) do if trip.id == self.selected then chosen = trip end end
    if self.selected ~= nil and chosen == nil then self:clear() end
    local parts = {data.world, tostring(data.shown), tostring(width), tostring(height), tostring(self.selected)}
    local ids = {}
    for _, trip in ipairs(data.trips) do
      local x, y = trip.position[1] - 10 * P, trip.position[2] - 10 * P
      if x < width and y < height and x + 21 * P > 0 and y + 22 * P > 0 then
        ids[#ids + 1] = trip.id
      end
      parts[#parts + 1] = trip.id .. ":" .. trip.icon .. ":" .. trip.position[1] .. ":" .. trip.position[2]
      if trip.id == self.selected then
        for _, paths in ipairs({trip.past, trip.ahead}) do
          parts[#parts + 1] = "paths"
          for _, path in ipairs(paths) do
            parts[#parts + 1] = "path"
            for _, point in ipairs(path) do parts[#parts + 1] = point[1] .. ":" .. point[2] end
          end
        end
      end
    end
    local signature = table.concat(parts, ";")
    if signature == self.signature then return end
    self.signature = signature
    local membership = table.concat(ids, ",")
    if membership ~= self.membership then
      for id in pairs(self.hits) do remove_hit(id) end
      self.membership = membership
    end
    clear_drawn()
    if chosen then
      for _, pair in ipairs({{chosen.past, "#a0a0a0"}, {chosen.ahead, "#f0cd3c"}}) do
        for _, path in ipairs(pair[1]) do for i = 2, #path do line(path[i - 1], path[i], pair[2], width, height) end end
      end
    end
    for _, trip in ipairs(data.trips) do
      local x, y = trip.position[1] - 10 * P, trip.position[2] - 10 * P
      if x < width and y < height and x + 21 * P > 0 and y + 22 * P > 0 then
        quad("ui/expeditions/" .. trip.icon .. ".png", x, y, 21 * P, 22 * P, self.chips, "#ffffffff")
        local key = self.hits[trip.id]
        if key == nil then
          key = ui.key("expedition")
          self.hits[trip.id] = key
          local pressed = false
          pointer.register(key, {
          functional = function() return true end,
          on_hover = function() end,
          on_overflow = function() pressed = false end,
          on_event = function(_, event, eligible)
            if event.kind == "down" then pressed = true; pointer.pressed(key, true)
            elseif event.kind == "up" then
              if pressed and eligible and event.inside then self.selected = trip.id; self.signature = nil end
              pressed = false; pointer.pressed(key, false)
            elseif event.kind == "cancel" then pressed = false; pointer.pressed(key, false) end
          end,
          })
        end
        render.set_hit(key, {{x = x, y = y, width = 21 * P, height = 22 * P}}, self.chips)
      end
    end
  end
  return self
end
return M
