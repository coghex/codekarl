-- Shared tooltip definitions for place data. `main.lua` pins each lookup to the newest economy revision it has received.
local tooltip = require("lib.tooltip")

local classes = {}
local revision

function classes.set_revision(value)
  revision = value
end

local function place_view(id)
  if revision == nil or game.place == nil then return nil end
  return game.place(id, revision)
end

local function place_name(id)
  for _, place in ipairs(game.places()) do
    if place.id == id then return place.name end
  end
  return id
end

local function term(key, words)
  local entry = glossary.entry(key)
  if entry == nil then return {text = words or key, term = key} end
  return {text = words or entry.title, term = key}
end

local function linked(value, class, id)
  return {text = tostring(value), class = class, args = {place = id}}
end

local function number(value)
  return tostring(math.floor(value or 0))
end

local function limited_number(value, over_limit, suffix)
  local text = number(value) .. (suffix or "")
  if over_limit then return {text = text, over_limit = true} end
  return text
end

local function share(part, whole)
  if whole == 0 then return "0%" end
  return tostring(math.floor(part * 100 / whole)) .. "%"
end

local function square_kilometres(cells)
  return (string.format("%.2f", (cells or 0) / 4):gsub("0+$", ""):gsub("%.$", ""))
end

local function rounded_value(value_milli)
  return math.floor((value_milli or 0) / 1000 + 0.5)
end

local function people(pop)
  return (pop.workers or 0) + (pop.dependents or 0)
end

local function aggregate(pops, field)
  local totals = {}
  for _, pop in ipairs(pops) do
    local id = pop[field]
    totals[id] = (totals[id] or 0) + people(pop)
  end
  local rows = {}
  for id, count in pairs(totals) do rows[#rows + 1] = {id = id, people = count} end
  table.sort(rows, function(a, b)
    if a.people ~= b.people then return a.people > b.people end
    return a.id < b.id
  end)
  return rows
end

local function total_value(rows)
  local total = 0
  for _, row in ipairs(rows) do total = total + (row.value_milli or 0) end
  return rounded_value(total)
end

local function value_line(label, value, class, id)
  return {term(label), " last month ", value == nil and "-" or linked(value, class, id)}
end

tooltip.define("location", function(args)
  local id = args.place
  local place = place_view(id)
  if place == nil then return nil end
  local total = (place.workers or 0) + (place.dependents or 0)
  local lines = {
    {term("population"), " ", linked(total, "population", id)},
  }
  if #place.pops > 0 then
    local cultures = aggregate(place.pops, "culture")
    local religions = aggregate(place.pops, "religion")
    local culture, religion = cultures[1], religions[1]
    lines[#lines + 1] = {term("culture"), " ", term(culture.id), " ", linked(share(culture.people, total), "cultures", id)}
    lines[#lines + 1] = {term("religion"), " ", term(religion.id), " ", linked(share(religion.people, total), "religions", id)}
  end
  local usable_cells = 0
  for _, count in pairs(place.land.usable) do usable_cells = usable_cells + count end
  lines[#lines + 1] = {term("usable_land", "usable land"), " ", linked(square_kilometres(usable_cells), "land", id), " km²"}
  local closed = place.last_month.closed
  local produced = closed and total_value(place.last_month.produced) or nil
  local consumed = closed and total_value(place.last_month.used_by_pops) or nil
  lines[#lines + 1] = value_line("produced", produced, "produced", id)
  lines[#lines + 1] = value_line("consumed", consumed, "consumed", id)
  return {title = place_name(id), table.unpack(lines)}
end)

local plurals = {peasant = "peasants", labourer = "labourers", monk = "monks"}

tooltip.define("population", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local totals = {}
  for _, pop in ipairs(place.pops) do
    local id = pop.kind
    totals[id] = (totals[id] or 0) + people(pop)
  end
  local rows = {}
  for id, count in pairs(totals) do rows[#rows + 1] = {id = id, people = count} end
  table.sort(rows, function(a, b)
    if a.people ~= b.people then return a.people > b.people end
    return a.id < b.id
  end)
  local lines = {}
  for _, row in ipairs(rows) do
    lines[#lines + 1] = {term(row.id, plurals[row.id] or (row.id .. "s")), " ", number(row.people)}
  end
  lines[#lines + 1] = {term("worker", "workers"), " ", number(place.workers), ", ", term("dependent", "dependents"), " ", number(place.dependents)}
  return lines
end)

local function demographic(field, args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local rows = aggregate(place.pops, field)
  local total = (place.workers or 0) + (place.dependents or 0)
  local lines = {}
  for _, row in ipairs(rows) do
    lines[#lines + 1] = {term(row.id), " ", number(row.people), " (", share(row.people, total), ")"}
  end
  return lines
end

tooltip.define("cultures", function(args) return demographic("culture", args) end)
tooltip.define("religions", function(args) return demographic("religion", args) end)

local function terrain_rows(values)
  local rows = {}
  for id, count in pairs(values) do
    if count > 0 then rows[#rows + 1] = {id = id, count = count} end
  end
  table.sort(rows, function(a, b)
    if a.count ~= b.count then return a.count > b.count end
    return a.id < b.id
  end)
  return rows
end

tooltip.define("land", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local lines, land_cells, usable_cells = {}, 0, 0
  for _, row in ipairs(terrain_rows(place.land.usable)) do
    lines[#lines + 1] = {row.id, " ", square_kilometres(row.count)}
    usable_cells = usable_cells + row.count
  end
  for _, count in pairs(place.land.land) do land_cells = land_cells + count end
  local other_cells = land_cells - usable_cells
  if other_cells > 0 then lines[#lines + 1] = {"not usable ", square_kilometres(other_cells)} end
  if place.land.sea > 0 then lines[#lines + 1] = {"sea ", square_kilometres(place.land.sea)} end
  return lines
end)

local function goods_lines(rows, id)
  local ordered = {}
  for index, row in ipairs(rows) do
    if row.units > 0 then ordered[#ordered + 1] = {row = row, index = index} end
  end
  table.sort(ordered, function(a, b)
    local a_value, b_value = a.row.value_milli or 0, b.row.value_milli or 0
    if a_value ~= b_value then return a_value > b_value end
    return a.index < b.index
  end)
  local lines = { {"value at local prices:"} }
  for _, item in ipairs(ordered) do
    lines[#lines + 1] = {term(item.row.good), " ", number(rounded_value(item.row.value_milli))}
  end
  return lines
end

tooltip.define("produced", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  return goods_lines(place.last_month.produced, args.place)
end)

tooltip.define("consumed", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  return goods_lines(place.last_month.used_by_pops, args.place)
end)

tooltip.define("pop_size", function(args)
  local place = place_view(args.place)
  local pop = place ~= nil and place.pops[args.pop] or nil
  if pop == nil then return nil end
  return {
    {term("worker", "workers"), " ", number(pop.workers)},
    {term("dependent", "dependents"), " ", number(pop.dependents)},
    {"births ", number(pop.births), ", deaths ", number(pop.deaths)},
  }
end)

local function pop_view(args)
  local place = place_view(args.place)
  return place ~= nil and place.pops[args.pop] or nil
end

tooltip.define("pop_food", function(args)
  local pop = pop_view(args)
  if pop == nil then return nil end
  local lines = {}
  for _, food in ipairs(pop.food or {}) do
    local unit = food.unit or ""
    if food.units ~= 1 and unit ~= "head" and not unit:find(" ", 1, true) and unit:sub(-1) ~= "s" then
      unit = unit .. "s"
    end
    lines[#lines + 1] = {
      term(food.good), " ", number(food.units), unit == "" and "" or " " .. unit,
      ", ", number((food.share_pm or 0) // 10), "% of meals",
    }
  end
  return lines
end)

tooltip.define("jobs", function(args)
  local pop = pop_view(args)
  if pop == nil then return nil end
  return { {"employed ", number(pop.employed)}, {"unemployed ", number(pop.unemployed)} }
end)

tooltip.define("pop_housing", function(args)
  local pop = pop_view(args)
  if pop == nil then return nil end
  return {
    {term("basic_housing", "basic"), " ", number(pop.basic_housing)},
    {term("makeshift_housing", "makeshift"), " ", number(pop.makeshift_housing)},
  }
end)

tooltip.define("pop_crowding", function(args)
  local pop = pop_view(args)
  if pop == nil then return nil end
  local crowding = pop.crowding_pm
  local crowding_text = (crowding == nil or crowding == false) and "—"
    or limited_number(crowding // 10, crowding > 1000, "%")
  return {
    {term("crowding"), " ", crowding_text},
    {term("housing"), " ", number((pop.basic_housing or 0) + (pop.makeshift_housing or 0))},
  }
end)

local function building_view(args)
  local place = place_view(args.place)
  return place ~= nil and place.buildings[args.building] or nil
end

tooltip.define("staff", function(args)
  local building = building_view(args)
  if building == nil then return nil end
  return {{"filled ", number(building.filled)}, {"open ", number(building.open)}}
end)

tooltip.define("store", function(args)
  local building = building_view(args)
  if building == nil then return nil end
  return {{"used ", limited_number(building.store_used_l, building.store_used_l > building.store_capacity_l),
    " L of ", number(building.store_capacity_l), " L"}}
end)

local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}

local function wage_value(wage_milli)
  return string.format("%.1f", (wage_milli or 0) / 1000)
end

tooltip.define("wage", function(args)
  local building = building_view(args)
  if building == nil then return nil end
  local lines = {}
  for _, wage in ipairs(building.wages or {}) do
    local month = MONTHS[wage.month]
    if month ~= nil then
      local prefix = wage.year == nil and month .. ": " or month .. " " .. number(wage.year) .. ": "
      local suffix = wage.paid == nil and "" or " (" .. number(wage.paid) .. " paid)"
      lines[#lines + 1] = {prefix .. wage_value(wage.wage_milli) .. " per head" .. suffix}
    end
  end
  lines[#lines + 1] = {"average ", wage_value(building.wage)}
  return lines
end)

tooltip.define("herd", function(args)
  local building = building_view(args)
  if building == nil then return nil end
  for _, herd in ipairs(building.herds or {}) do
    if herd.species == args.species then
      return {{term(herd.species), " ", number(herd.head), " of ", number(herd.maximum)}}
    end
  end
  return nil
end)

tooltip.define("place_food", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local ordered = {}
  for index, pop in ipairs(place.pops) do
    ordered[#ordered + 1] = {pop = pop, index = index}
  end
  table.sort(ordered, function(a, b)
    if a.pop.food_days ~= b.pop.food_days then return a.pop.food_days < b.pop.food_days end
    return a.index < b.index
  end)
  local lines = {}
  for _, item in ipairs(ordered) do
    local pop = item.pop
    lines[#lines + 1] = {term(pop.kind), " ", term(pop.culture), " ", number(pop.food_days), " days"}
  end
  return lines
end)

tooltip.define("housing", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  return {
    {term("basic_housing", "basic"), " ", limited_number(place.basic_housing_occupants,
      place.basic_housing_occupants > place.basic_housing_capacity), " of ", number(place.basic_housing_capacity)},
    {term("makeshift_housing", "makeshift"), " ", limited_number(place.makeshift_housing_occupants,
      place.makeshift_housing_occupants > place.makeshift_housing_capacity), " of ", number(place.makeshift_housing_capacity)},
  }
end)

tooltip.define("crowding", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local people_count = (place.workers or 0) + (place.dependents or 0)
  local room = (place.basic_housing_capacity or 0) + (place.makeshift_housing_capacity or 0)
  return {
    {"people ", number(people_count)},
    {"room ", number(room)},
    {term("crowding"), " ", limited_number((place.crowding_pm or 0) // 10, (place.crowding_pm or 0) > 1000, "%")},
  }
end)

tooltip.define("building_land", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  return {
    {"in use ", limited_number(place.footprint_m2,
      place.footprint_m2 > math.min(place.prepared_m2, place.max_buildable_m2)), " m²"},
    {"prepared ", number(place.prepared_m2), " m²"},
    {"buildable ", number(place.max_buildable_m2), " m²"},
  }
end)

tooltip.define("storage", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local lines = {
    {term("pop", "pops"), " ", limited_number(place.storage_used_l,
      place.storage_used_l > place.storage_capacity_l), " / ", number(place.storage_capacity_l), " L"},
  }
  for _, building in ipairs(place.buildings) do
    if (building.store_capacity_l or 0) > 0 then
      lines[#lines + 1] = {term(building.kind), " ", limited_number(building.store_used_l,
        building.store_used_l > building.store_capacity_l), " / ", number(building.store_capacity_l), " L"}
    end
  end
  return lines
end)

tooltip.define("overflow", function(args)
  local place = place_view(args.place)
  if place == nil then return nil end
  local ordered = {}
  for index, row in ipairs(place.last_month.lost) do
    ordered[#ordered + 1] = {row = row, index = index}
  end
  table.sort(ordered, function(a, b)
    if a.row.units ~= b.row.units then return a.row.units > b.row.units end
    return a.index < b.index
  end)
  if #ordered == 0 then return {{"nothing lost"}} end
  local lines = {}
  for _, item in ipairs(ordered) do
    lines[#lines + 1] = {term(item.row.good), " ", limited_number(item.row.units, item.row.units > 0)}
  end
  return lines
end)

return classes
