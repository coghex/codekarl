-- The layout of one graph's chart (CK-441, spec r5 rules 35 to 41): from the answer `game.history_graph` gave, the filled rectangles of its
-- axes, graph forms, labels, its legend and the text of a column's tooltip. It makes no scene call: the graph window draws
-- what this returns, and the tests read it. Every position is in UI pixels from the chart area's top-left corner.
local draw = require("lib.draw")

local chart = {}

-- The y axis' labels take this much room at the left of the plot, and the legend this much at its right (rule 39).
chart.GUTTER = 30
chart.LEGEND = 72
chart.LEGEND_GAP = 6
-- A legend is left out when the plot would be narrower than this (rule 39).
chart.LEAST_PLOT = 160
-- A chart has at most this many columns (rule 35).
chart.MAX_COLUMNS = 250
local TOP = 5 -- room above the plot for the top label
local STRIP = 12 -- the years under the plot
local RIGHT = 3

local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}

-- The longest history there is, in centuries (768 to 9999): the columns a chart can need at its coarsest step (rule 35).
chart.LONGEST = 93

-- The least plot the legend may leave (rule 39): 160 pixels, and for a graph whose series stand side by side (`per` of them in a column) room for
-- every one of the longest history's columns, a pixel to each series, so that none is lost.
function chart.least_plot(per)
  return math.max(chart.LEAST_PLOT, (per or 1) * chart.LONGEST)
end

-- The plot's width and whether the legend is shown, for a chart area `width` wide: the legend is left out when the plot would then be
-- narrower than `least_plot(per)` (rule 39). `per` is the series side by side in a column (1 when they stack or there is one).
function chart.plot_width(width, per)
  local x0 = chart.GUTTER + 2
  local with_legend = width - x0 - chart.LEGEND - chart.LEGEND_GAP - RIGHT
  if with_legend >= chart.least_plot(per) then return with_legend, true end
  return math.max(0, width - x0 - RIGHT), false
end

-- The plot's column budget (rule 35): the smaller of 250 and its pixels, a column needing a pixel for each series that stands beside another.
function chart.budget(width, per)
  per = per or 1
  local plot = chart.plot_width(width, per)
  return math.max(0, math.min(chart.MAX_COLUMNS, plot // per))
end

local NICE = {1, 1.2, 1.6, 2, 2.4, 3.2, 4, 6, 8, 10}

-- A round maximum just above `value` (rule 37).
function chart.round_max(value)
  if value <= 0 then return 4 end
  local exponent = math.floor(math.log(value, 10))
  local scale = 10 ^ exponent
  local mantissa = value / scale
  for _, candidate in ipairs(NICE) do
    if candidate >= mantissa - 1e-9 then return candidate * scale end
  end
  return 10 * scale
end

-- A number as an axis label or a tooltip figure: whole numbers as they are, others with at most two decimals.
function chart.format(value)
  if value == math.floor(value) then return string.format("%d", value) end
  local text = string.format("%.2f", value)
  text = text:gsub("0+$", ""):gsub("%.$", "")
  return text
end

local function round(value) return math.floor(value + 0.5) end

local function total_of(column, stacked)
  local result = 0
  for _, value in ipairs(column.values or {}) do
    if type(value) == "number" then
      if stacked then result = result + value else result = math.max(result, value) end
    end
  end
  return result
end

-- The chart for `answer` in a chart area `spec.width` by `spec.height` UI pixels. `spec.colour_of(index, key)` is the colour of a series and
-- `spec.name_of(key)` its name; `spec.scale` (default 1) multiplies every value (a price's thousandths).
function chart.layout(answer, spec)
  local scale = spec.scale or 1
  local per = spec.per or 1
  if answer ~= nil and answer.series ~= nil then per = answer.stacked and 1 or #answer.series end
  local plot_w, legend = chart.plot_width(spec.width, per)
  local x0 = chart.GUTTER + 2
  local plot_h = math.max(1, spec.height - TOP - STRIP)
  local out = {rects = {}, labels = {}, legend = {}, columns = {}, plot = {x = x0, y = TOP, w = plot_w, h = plot_h}, legend_shown = legend}
  if answer == nil then return out end
  if answer.message ~= nil then out.message = answer.message; return out end
  local columns = answer.columns
  local count = #columns
  if count == 0 or plot_w <= 0 then return out end
  local dim, track = spec.dim, spec.track
  local largest = 0
  for _, column in ipairs(columns) do largest = math.max(largest, total_of(column, answer.stacked and spec.form ~= "lines") * scale) end
  local maximum = chart.round_max(largest)
  out.maximum = maximum
  local base = TOP + plot_h
  local function add(x, y, w, h, colour)
    if w > 0 and h > 0 then out.rects[#out.rects + 1] = {x = x, y = y, w = w, h = h, colour = colour} end
  end
  for quarter = 1, 4 do add(x0, base - (plot_h * quarter) // 4, plot_w, 1, track) end
  out.labels[#out.labels + 1] = {x = x0 - 3, y = base - 4, text = "0", align = "right", colour = dim}
  out.labels[#out.labels + 1] = {x = x0 - 3, y = base - plot_h // 2 - 4, text = chart.format(maximum / 2), align = "right", colour = dim}
  out.labels[#out.labels + 1] = {x = x0 - 3, y = TOP - 4, text = chart.format(maximum), align = "right", colour = dim}
  local edges = {}
  for index = 0, count do edges[index] = x0 + (index * plot_w) // count end
  local series = answer.series
  for index, column in ipairs(columns) do
    local left, cell = edges[index - 1], edges[index] - edges[index - 1]
    local gap = cell >= 3 and 1 or 0
    out.columns[index] = {x = left, w = cell}
    if column.record then
      -- The axis line is present wherever the month was recorded, a measured zero included (rule 36).
      add(left, base, cell, 1, dim)
      local width = cell - gap
      if spec.form ~= nil and spec.form ~= "columns" then
        -- Other forms are drawn after the axes, joining column centres.
      elseif answer.stacked then
        local acc = 0
        for number, value in ipairs(column.values) do
          if type(value) == "number" then
            local low, high = round(acc / maximum * plot_h), round((acc + value * scale) / maximum * plot_h)
            add(left, base - high, width, high - low, spec.colour_of(number, series[number]))
            acc = acc + value * scale
          end
        end
      else
        if width >= #series then
          -- Side by side, each series in a slot of its own.
          local slot = width // #series
          for number, value in ipairs(column.values) do
            if type(value) == "number" then
              local height = round(value * scale / maximum * plot_h)
              add(left + (number - 1) * slot, base - height, slot, height, spec.colour_of(number, series[number]))
            end
          end
        else
          -- A column narrower than its series (a long range at the least window, rule 35 allows one pixel a column): every series is drawn over the
          -- whole column, the tallest first, so a shorter one shows in front of it and no positive value is lost.
          local order = {}
          for number, value in ipairs(column.values) do
            if type(value) == "number" then order[#order + 1] = {number = number, value = value} end
          end
          table.sort(order, function(a, b)
            if a.value ~= b.value then return a.value > b.value end
            return a.number < b.number
          end)
          for _, entry in ipairs(order) do
            local height = round(entry.value * scale / maximum * plot_h)
            add(left, base - height, width, height, spec.colour_of(entry.number, series[entry.number]))
          end
        end
      end
    end
  end
  if spec.form == "lines" or (spec.form == "bands" and answer.stacked) then
    local function centre(index) return (edges[index - 1] + edges[index] - 1) // 2 end
    local function value(index, number)
      local column = columns[index]
      if column == nil or not column.record then return nil end
      local v = column.values[number]
      if type(v) ~= "number" then return nil end
      return v * scale
    end
    local function bounds(index, number)
      local v = value(index, number)
      if v == nil then return nil end
      local low = 0
      if spec.form == "bands" then
        for previous = 1, number - 1 do low = low + (value(index, previous) or 0) end
      end
      return low / maximum * plot_h, (low + v) / maximum * plot_h
    end
    for number, key in ipairs(series) do
      local colour = spec.colour_of(number, key)
      for index = 1, count do
        local low, high = bounds(index, number)
        if low ~= nil then
          local prev = index > 1 and value(index - 1, number) ~= nil
          local next_value = index < count and value(index + 1, number) ~= nil
          local x = centre(index)
          if not prev and not next_value then
            local left = math.max(x0, x - 1)
            local width = math.min(x0 + plot_w, x + 2) - left
            if spec.form == "lines" then add(left, base - round(high), width, 1, colour)
            else add(left, base - round(high), width, round(high) - round(low), colour) end
          end
          if next_value then
            local next_low, next_high = bounds(index + 1, number)
            local nx = centre(index + 1)
            if spec.form == "lines" then
              local last_y = base - round(high)
              for px = x, nx do
                local t = nx == x and 0 or (px - x) / (nx - x)
                local y = base - round(high + (next_high - high) * t)
                add(px, math.min(y, last_y), 1, math.abs(y - last_y) + 1, colour)
                last_y = y
              end
            else
              for px = x, nx do
                local t = nx == x and 0 or (px - x) / (nx - x)
                local lo = round(low + (next_low - low) * t)
                local hi = round(high + (next_high - high) * t)
                add(px, base - hi, 1, hi - lo, colour)
              end
            end
          end
        end
      end
    end
  end
  -- The years under the columns, as many as fit without overlapping (rule 37).
  local last_end = -1000
  local previous
  for index, column in ipairs(columns) do
    local year = column.from.year
    if index == 1 or year ~= previous then
      local label = tostring(year)
      local x = edges[index - 1]
      if x >= last_end + 4 and x + draw.width(label, "small") <= x0 + plot_w + RIGHT then
        out.labels[#out.labels + 1] = {x = x, y = base + 3, text = label, align = "left", colour = dim}
        last_end = x + draw.width(label, "small")
      end
    end
    previous = year
  end
  if legend then
    local x = x0 + plot_w + chart.LEGEND_GAP
    for index, key in ipairs(series) do
      local y = TOP + (index - 1) * 10
      out.legend[#out.legend + 1] = {x = x, y = y, swatch = spec.colour_of(index, key), text = spec.name_of(key)}
    end
  end
  return out
end

-- The column at x (UI pixels from the chart area's left) or nil.
function chart.column_at(layout, x)
  local plot = layout.plot
  if x < plot.x or x >= plot.x + plot.w then return nil end
  for index, column in ipairs(layout.columns) do
    if x >= column.x and x < column.x + column.w then return index end
  end
  return nil
end

local function month_text(date) return MONTHS[date.month] .. " " .. date.year end

-- A column's period in words (rule 40).
function chart.period(column, step)
  local from, to = column.from, column.to
  if from.year == to.year and from.month == to.month then return month_text(from) end
  if step == "year" or step == "decade" or step == "quarter century" or step == "century" then
    if from.year == to.year then return tostring(from.year) end
    return from.year .. " to " .. to.year
  end
  if from.year == to.year then return MONTHS[from.month] .. " to " .. MONTHS[to.month] .. " " .. from.year end
  return month_text(from) .. " to " .. month_text(to)
end

-- The lines of the tooltip of column `index` (rule 40): its period, then each series' value and, for a rate, its count and average people.
function chart.tooltip(answer, index, spec)
  local column = answer.columns[index]
  if column == nil then return nil end
  local scale = spec.scale or 1
  local lines = {title = chart.period(column, answer.step)}
  if not column.record then lines[#lines + 1] = "no record"; return lines end
  for number, key in ipairs(answer.series) do
    local name, value = spec.name_of(key), column.values[number]
    if answer.rate then
      local count = column.counts[number]
      if type(value) ~= "number" then
        lines[#lines + 1] = count .. " " .. name .. ", no people"
      else
        lines[#lines + 1] = name .. " " .. chart.format(math.floor(value * 10 + 0.5) / 10) .. " a year per 1,000 ("
          .. count .. " " .. name .. " of " .. round(column.people) .. " people)"
      end
    elseif type(value) == "number" then
      lines[#lines + 1] = name .. " " .. chart.format(math.floor(value * scale * 100 + 0.5) / 100)
    else
      lines[#lines + 1] = name .. " no value"
    end
  end
  return lines
end

return chart
