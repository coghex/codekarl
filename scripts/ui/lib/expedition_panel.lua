-- The selected expedition's left-slot panel, bound to its number and world.
local ui = require('lib.ui')
local windows = require('lib.window')
local location = require('lib.location')
local cards = require('lib.cards')
local tabs = require('lib.tabs')
local draw = require('lib.draw')
local theme = require('lib.theme')
local scrollbar = require('lib.scrollbar')
local place_names = require('lib.place_names')
local pointer = require('lib.pointer')
local tooltip = require('lib.tooltip')
local P = ui.P
local panel = {}
panel.__index = panel

local function item(height, paint, click)
  return {height = height, draw = function(group, _, y)
    local view = draw.view()
    paint(view, group, y // P)
    return view
  end, on_click = click}
end
local function name(id) return (id or ''):gsub('_', ' ') end
local function place(id) return place_names.name_of(id) or name(id) end
local function pop_colour(kind)
  local ok, colour = pcall(theme.colour, 'pop_' .. tostring(kind))
  return ok and colour or theme.colour('text')
end
local function heading(view, group, y, width, label)
  local used = draw.text(view, group, 0, y, label, theme.kind('expedition').divider, 'small')
  draw.rect(view, group, used + 3, y + 4, math.max(1, width - used - 3), 1, theme.kind('expedition').outline)
end
local function line(view, group, y, width, left, right)
  local clipped_right = draw.clip(right, width)
  local room = math.max(0, width - draw.width(clipped_right) - (clipped_right ~= '' and 4 or 0))
  draw.text(view, group, 0, y, draw.clip(left, room), theme.colour('term'))
  draw.text(view, group, width, y, clipped_right, theme.colour('text'), nil, 'right')
end
local function wrap(content, width)
  local rows, current = {}, ''
  for word in content:gmatch('%S+') do
    local joined = current == '' and word or (current .. ' ' .. word)
    if current ~= '' and draw.width(joined) > width then rows[#rows + 1] = current; current = word else current = joined end
  end
  if current ~= '' then rows[#rows + 1] = current end
  return rows
end
local function amount(value, denominator, unit)
  if denominator == 0 then return '—' end
  local scale = unit == 'm³' and 1000000 or 1000
  return string.format('%.1f / %.1f %s', value / scale, denominator / scale, unit)
end
local function bar(view, group, x, y, width, value, maximum)
  draw.rect(view, group, x, y, width, 3, theme.colour('track'))
  draw.rect(view, group, x, y, draw.bar_fill(value, maximum, width), 3, theme.colour('number'))
end
local function vehicle_icon(view, group, kind, x, y)
  local quad = render.create_quad{texture = ui.texture('ui/expeditions/title_' .. kind .. '.png'), x = x * P, y = y * P,
    width = 13 * P, height = 11 * P, group = group, tint = theme.kind('expedition').title_text}
  view:add({destroy = function() render.destroy_quad(quad) end})
end
local goods_art = {}
for _, id in ipairs({'oats','barley','peas_beans','vegetables','apples','dairy','beef','pork','mutton','fish','seabirds',
  'ale','mead','wine','honey','herbs','wool','beeswax','hides','linen','hemp','hardwood','peat','cattle','pigs','sheep','horses'}) do goods_art[id] = true end
local function good_icon(view, group, id, x, y)
  local path = goods_art[id] and id or 'fallback/unknown'
  local quad = render.create_quad{texture = ui.texture('ui/goods/' .. path .. '.png'), x = x * P, y = y * P,
    width = 9 * P, height = 9 * P, group = group}
  view:add({destroy = function() render.destroy_quad(quad) end})
end
local function list_section(title, goods, width, suffix)
  if #goods == 0 then return nil end
  return item(12 + #goods * 11, function(view, group, y)
    heading(view, group, y, width, title)
    for i, good in ipairs(goods) do
      local mark = suffix and suffix(good) or ''
      good_icon(view, group, good.id, 0, y + i * 11)
      local figure = tostring(good.amount) .. mark
      draw.text(view, group, 12, y + i * 11, draw.clip(name(good.id), math.max(0, width - 16 - draw.width(figure))), theme.colour('term'))
      draw.text(view, group, width, y + i * 11, figure, theme.colour('text'), nil, 'right')
    end
  end)
end
local function add(items, value) if value then items[#items + 1] = value end end
local function overview(data, width)
  local items = {}
  local sender = item(11, function(v, g, y)
    local used = draw.text(v, g, 0, y, name(data.trader), pop_colour(data.trader))
    draw.text(v, g, used, y, draw.clip(' of ' .. place(data.home), math.max(0, width - used)), theme.colour('term'))
  end)
  sender.tooltip = function(x, y)
    local left = draw.width(name(data.trader)) + draw.width(' of ')
    if y >= 0 and y < 11 and x >= left and x < left + draw.width(place(data.home)) then return tooltip.content('location', {place = data.home}) end
  end
  add(items, sender)
  local journey = item(56, function(v, g, y)
    heading(v, g, y, width, 'journey')
    line(v, g, y + 11, width, 'to ' .. place(data.destination), data.phase)
    line(v, g, y + 22, width, 'travelled', data.travelled .. ' / ' .. data.total .. ' h')
    bar(v, g, math.floor(width * .35), y + 26, math.floor(width * .2), data.travelled, data.total)
    line(v, g, y + 33, width, 'arrives', data.arrives_text)
    line(v, g, y + 44, width, 'trades', data.trades_text)
  end)
  journey.tooltip = function(x, y)
    local left = draw.width('to ')
    if y >= 11 and y < 22 and x >= left and x < left + draw.width(place(data.destination)) then return tooltip.content('location', {place = data.destination}) end
  end
  add(items, journey)
  local count, crew = 0, 0
  for _, group in ipairs(data.people) do count = count + group.count end
  for _, vehicle in ipairs(data.vehicles) do crew = crew + vehicle.crew end
  add(items, item(34, function(v, g, y)
    heading(v, g, y, width, 'people')
    line(v, g, y + 11, width, tostring(count) .. ' people', tostring(data.food_days) .. ' food days')
    line(v, g, y + 22, width, tostring(crew) .. ' crew', tostring(data.drovers) .. ' drovers')
  end))
  add(items, item(45, function(v, g, y)
    heading(v, g, y, width, 'pace')
    line(v, g, y + 11, width, 'pace', string.format('%.3g · %s', data.pace_pm / 1000, name(data.limiter)))
    line(v, g, y + 22, width, 'weight', amount(data.load_g, data.capacity_g, 'kg'))
    line(v, g, y + 33, width, 'space', amount(data.load_ml, data.capacity_ml, 'm³'))
    bar(v, g, math.floor(width * .35), y + 26, math.floor(width * .2), data.load_g, data.capacity_g)
    bar(v, g, math.floor(width * .35), y + 37, math.floor(width * .2), data.load_ml, data.capacity_ml)
  end))
  add(items, item(31 + #data.cargo * 11, function(v, g, y)
    heading(v, g, y, width, 'cargo')
    draw.rect(v, g, 0, y + 11, width, 3, theme.colour('track'))
    local cursor = 0
    for _, good in ipairs(data.cargo) do
      local weight = (good.animal and not data.water) and 0 or good.weight_g
      local next_pos = draw.bar_fill(cursor + weight, data.capacity_g, width)
      local left = draw.bar_fill(cursor, data.capacity_g, width)
      draw.rect(v, g, left, y + 11, next_pos - left, 3, theme.colour('number'))
      cursor = cursor + weight
    end
    local all = draw.bar_fill(data.load_g, data.capacity_g, width)
    local before = draw.bar_fill(cursor, data.capacity_g, width)
    draw.rect(v, g, before, y + 11, all - before, 3, theme.colour('dim'))
    line(v, g, y + 17, width, 'load', amount(data.load_g, data.capacity_g, 'kg'))
    for i, good in ipairs(data.cargo) do
      local mark
      if good.animal then mark = data.water and ' · aboard' or ' · driven'
      else mark = data.capacity_g == 0 and ' · —' or string.format(' · %d%%', good.weight_g * 100 // data.capacity_g) end
      good_icon(v, g, good.id, 0, y + 20 + i * 11)
      local figure = tostring(good.amount) .. mark
      draw.text(v, g, 12, y + 20 + i * 11, draw.clip(name(good.id), math.max(0, width - 16 - draw.width(figure))), theme.colour('term'))
      draw.text(v, g, width, y + 20 + i * 11, figure, theme.colour('text'), nil, 'right')
    end
  end))
  add(items, list_section('to trade for', data.wanted, width))
  add(items, list_section('supplies', data.supplies, width))
  return items
end
local function crew(data, width)
  local items = {}
  for _, person in ipairs(data.people) do
    local paid_lines = person.own and {'own people, unpaid'} or {'hired, paid'}
    if not person.own then for _, wage in ipairs(person.paid) do
      local text = wage.amount .. ' ' .. name(wage.id)
      local last = #paid_lines
      local joined = paid_lines[last] .. ' ' .. text
      if draw.width(joined) <= width - 8 then paid_lines[last] = joined else paid_lines[#paid_lines + 1] = text end
    end end
    local roles = {}; for _, role in ipairs(person.roles) do roles[#roles + 1] = role.amount .. ' ' .. name(role.role) end
    local role_lines = wrap(table.concat(roles, ', '), width - 8)
    local height = 27 + (#paid_lines + #role_lines) * 11
    items[#items + 1] = item(height, function(v, g, y)
      draw.frame(v, g, 0, y, width, height - 1, theme.kind('expedition').fill, theme.kind('expedition').outline)
      draw.text(v, g, 4, y + 3, name(person.kind), pop_colour(person.kind))
      draw.text(v, g, width - 4, y + 3, person.count .. ' people', theme.colour('text'), nil, 'right')
      local row = y + 15
      for _, text in ipairs(paid_lines) do draw.text(v, g, 4, row, draw.clip(text, width - 8), theme.colour('dim')); row = row + 11 end
      for _, text in ipairs(role_lines) do draw.text(v, g, 4, row, draw.clip(text, width - 8), theme.colour('text')); row = row + 11 end
      draw.text(v, g, width - 4, row, 'away ' .. math.max(0, (data.now - data.left) // 24) .. ' days', theme.colour('dim'), nil, 'right')
    end)
  end
  return items
end
local function vehicles(data, width, open)
  local items = {}
  if #data.vehicles == 0 then
    items[1] = item(47, function(v, g, y)
      draw.frame(v, g, 0, y, width, 46, theme.kind('expedition').fill, theme.kind('expedition').outline)
      vehicle_icon(v, g, 'on_foot', 3, y + 2)
      draw.text(v, g, 18, y + 3, 'on foot', theme.colour('term'))
      line(v, g, y + 15, width - 4, 'weight', amount(data.load_g, data.capacity_g, 'kg'))
      line(v, g, y + 26, width - 4, 'space', amount(data.load_ml, data.capacity_ml, 'm³'))
    end)
  end
  for _, vehicle in ipairs(data.vehicles) do
    items[#items + 1] = item(58, function(v, g, y)
      draw.frame(v, g, 0, y, width, 57, theme.kind('expedition').fill, theme.kind('expedition').outline)
      local status = vehicle.expedition and (vehicle.carrier and 'carried' or 'moving') or vehicle.free and 'free' or 'reserved'
      vehicle_icon(v, g, vehicle.kind, 3, y + 2)
      draw.text(v, g, 18, y + 3, draw.clip(name(vehicle.kind), width - 22), theme.colour('term'))
      line(v, g, y + 15, width - 4, status, amount(vehicle.load_g, vehicle.capacity_g, 'kg'))
      line(v, g, y + 26, width - 4, 'space', amount(vehicle.load_ml, vehicle.capacity_ml, 'm³'))
      local carrier
      if vehicle.carrier then for _, other in ipairs(data.vehicles) do if other.number == vehicle.carrier then carrier = name(other.kind) end end end
      line(v, g, y + 37, width - 4, vehicle.carrier and ('in ' .. (carrier or vehicle.carrier)) or ('crew ' .. vehicle.crew .. ' / ' .. vehicle.full_crew),
        vehicle.pace_pm and string.format('pace %.3g', vehicle.pace_pm / 1000) or '')
    end, function() open(vehicle.number) end)
  end
  return items
end
function panel.vehicle_items(data, width, open)
  if #data.vehicles == 0 then return {} end
  return vehicles(data, width, open)
end
function panel.new(spec) return setmetatable({view = spec.view, top = spec.top, on_close = spec.on_close, on_go = spec.on_go, on_follow = spec.on_follow, on_child = spec.on_child or function() end}, panel) end
function panel:is_open() return self.window ~= nil and not self.window.closed end
function panel:close() if self:is_open() then self.window:close() end end
function panel:refresh(data)
  if not self:is_open() then return end
  self.data = data
  if self.mode == 'expedition' and data.icon and data.icon ~= self.icon_name then
    if self.icon_quad then render.destroy_quad(self.icon_quad) end
    self.icon_name = data.icon
    self.icon_quad = render.create_quad{texture = ui.texture('ui/expeditions/title_' .. data.icon .. '.png'),
      x = self.tabs.buttons[1].x + 2 * P, y = 2 * P, width = 13 * P, height = 11 * P,
      group = self.window.chrome, tint = theme.kind('expedition').title_text}
  end
  local width = self.window.content_width // P - scrollbar.RESERVE // P
  local items
  if self.mode == 'vehicle' then
    local vehicle = self.vehicle
    for _, candidate in ipairs(data.vehicles) do if candidate.number == vehicle.number then vehicle = candidate; self.vehicle = candidate end end
    items = {item(154, function(v, g, y)
      heading(v, g, y, width, 'vehicle')
      line(v, g, y + 11, width, 'kind', name(vehicle.kind))
      line(v, g, y + 22, width, 'owner', name(vehicle.owner) .. ' of ' .. place(data.home))
      local status = vehicle.expedition and ('with expedition ' .. vehicle.expedition) or vehicle.free and 'free' or 'reserved'
      line(v, g, y + 33, width, 'status', status)
      line(v, g, y + 44, width, 'capacity', string.format('%.1f kg', vehicle.capacity_g / 1000))
      line(v, g, y + 55, width, 'space', string.format('%.1f m³', vehicle.capacity_ml / 1000000))
      line(v, g, y + 66, width, 'crew', vehicle.crew_fewest .. '–' .. vehicle.full_crew)
      line(v, g, y + 77, width, 'carried weight', vehicle.carried_g and string.format('%.1f kg', vehicle.carried_g / 1000) or '—')
      line(v, g, y + 88, width, 'carried space', vehicle.carried_ml and string.format('%.1f m³', vehicle.carried_ml / 1000000) or '—')
      line(v, g, y + 99, width, 'land pace', vehicle.land_pace_pm and string.format('%.3g', vehicle.land_pace_pm / 1000) or '—')
      line(v, g, y + 110, width, 'water', vehicle.water_profile and name(vehicle.water_profile) or 'carried')
      line(v, g, y + 121, width, 'build', vehicle.build_hours .. ' h · ' .. vehicle.build_hands .. ' hands')
      line(v, g, y + 132, width, 'wear', name(vehicle.wear))
    end)}
    if vehicle.materials == false then
      items[#items + 1] = item(22, function(v, g, y) heading(v, g, y, width, 'built from'); draw.text(v, g, 0, y + 11, 'given at the start', theme.colour('text')) end)
    else add(items, list_section('built from', vehicle.materials, width)) end
  else
    items = self.tab == 'crew' and crew(data, width) or self.tab == 'vehicles' and vehicles(data, width, function(n)
      pointer.defer(function() if self:is_open() and self.mode == 'expedition' then self:open_vehicle(n) end end)
    end) or overview(data, width)
  end
  if self.stack then local pos = self.stack:scroll_position(); self.stack:set_items(items); self.stack:set_pos(pos)
  else self.stack = cards.new{window = self.window, x = 0, y = 0, width = self.window.content_width, height = self.window.content_height, gap = 3, items = items} end
end
function panel:open(id, world, data, selected)
  self:close()
  self.id, self.world, self.tab, self.mode, self.ended, self.origin_pop = id, world, selected or 'journey', 'expedition', false, false
  local top, width, height = location.slot(self.view, ui.snap(self.top()))
  local made
  made = windows.new{title = '', kind = 'expedition', expedition_owned = true, outer_width = width, outer_height = height, x = 0, y = top, draggable = false,
    on_close = function() if self.window == made then self.window, self.stack, self.follow_button, self.icon_quad, self.icon_name = nil, nil, nil, nil, nil; if not self.switching then self.on_close() end end end}
  self.window = made
  self.tabs = tabs.new{window = made, align = 'left', items = {
    {key = 'journey', label = '    ' .. place(data.destination), short = '    …', tiny = '    '},
    {key = 'crew', label = 'crew', short = 'crew', tiny = 'c'}, {key = 'vehicles', label = 'vehicles', short = 'veh', tiny = 'v'}},
    selected = self.tab, on_select = function(key) self.tab = key; self:refresh(self.data) end}
  made:add_title_item{place_in_title = function() if self.icon_quad then render.set_position(self.icon_quad, self.tabs.buttons[1].x + 2 * P, 2 * P) end end}
  made:add_title_button{icon = 'location', tooltip = 'go to', on_click = self.on_go}
  self.follow_button = made:add_title_button{icon = 'follow', tooltip = 'follow', on_click = self.on_follow}
  self:refresh(data)
end
function panel:set_following(value) if self.follow_button and self:is_open() then self.follow_button:set_selected(value) end end
function panel:relabel()
  if self:is_open() and self.mode == 'expedition' then
    self.tabs:set_labels('journey', '    ' .. place(self.data.destination), '    …', '    ')
    self:refresh(self.data)
  end
end
function panel:is_vehicle() return self:is_open() and self.mode == 'vehicle' end
function panel:open_vehicle(number)
  local vehicle
  for _, candidate in ipairs(self.data.vehicles) do if candidate.number == number then vehicle = candidate; break end end
  if not vehicle then return end
  self.on_child()
  self.switching = true; self:close(); self.switching = false
  self.mode, self.vehicle = 'vehicle', vehicle
  local top, width, height = location.slot(self.view, ui.snap(self.top()))
  local made
  made = windows.new{title = name(vehicle.kind), kind = 'expedition', expedition_owned = true, outer_width = width, outer_height = height, x = 0, y = top, draggable = false,
    on_close = function()
      if self.window ~= made then return end
      self.window, self.stack = nil, nil
      if self.switching then return end
      if self.origin_pop then self.return_to_pop()
      elseif not self.ended then self:open(self.id, self.world, self.data, 'vehicles') else self.on_close() end
    end}
  self.window = made
  self:refresh(self.data)
end
function panel:open_from_pop(vehicle, home, world, return_to_pop)
  self:discard()
  self.id, self.world, self.origin_pop, self.return_to_pop, self.ended = vehicle.expedition or '', world, true, return_to_pop, false
  self.data = {home = home, vehicles = {vehicle}}
  self:open_vehicle(vehicle.number)
end
function panel:trip_ended()
  if self:is_vehicle() then self.ended = true end
end
function panel:discard()
  self.switching = true; self:close(); self.switching = false; self.on_close()
end
function panel:relayout()
  if not self:is_open() then return end
  local top, width, height = location.slot(self.view, ui.snap(self.top()))
  if self.stack then self.stack:destroy(); self.stack = nil end
  self.window:set_outer_size(width, height); self.window:move_to(0, top); self:refresh(self.data)
end
return panel
