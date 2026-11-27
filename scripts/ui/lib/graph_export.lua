-- A CSV is made from the very answer the window draws, without consulting the live history.
local export = {}

local function field(value)
  local text = tostring(value)
  if text:find('[,"\r\n]') then return '"' .. text:gsub('"', '""') .. '"' end
  return text
end

local function date(value) return string.format('%04d-%02d', value.year, value.month) end
local function number(value, places)
  local factor = 10 ^ places
  local rounded = value < 0 and math.ceil(value * factor - 0.5) or math.floor(value * factor + 0.5)
  return string.format('%.' .. places .. 'f', rounded / factor)
end

function export.table(answer, graph, name_of)
  local header = {'from', 'to'}
  for _, key in ipairs(answer.series) do header[#header + 1] = field(name_of(key)) end
  if answer.rate then
    header[#header + 1] = 'people'
    for _, key in ipairs(answer.series) do header[#header + 1] = field(name_of(key) .. ' count') end
  end
  local rows = {table.concat(header, ',')}
  local places = (graph == 'made' or graph == 'used' or graph:match('^price:')) and 3 or 1
  local scale = graph:match('^price:') and 0.001 or 1
  for _, column in ipairs(answer.columns) do
    local row = {date(column.from), date(column.to)}
    for index = 1, #answer.series do
      local value = column.record and column.values[index] or nil
      row[#row + 1] = type(value) == 'number' and number(value * scale, places) or ''
    end
    if answer.rate then
      row[#row + 1] = column.record and number(column.people, 1) or ''
      for index = 1, #answer.series do row[#row + 1] = column.record and number(column.counts[index], 0) or '' end
    end
    rows[#rows + 1] = table.concat(row, ',')
  end
  return table.concat(rows, '\n') .. '\n'
end

function export.name(answer, subject, graph)
  return subject .. '-' .. graph .. '-' .. date(answer.columns[1].from) .. '-to-' .. date(answer.columns[#answer.columns].to)
end

return export
