-- The history graph windows (CK-441, history spec r5 part B): one graph a window, opened from the graph icons of the location tab or the time
-- bar's graph button, each with its three columns (tags, places, graphs), its range buttons and its stacked columns.
--
-- The windows are asked for through `game.history_subjects()` and `game.history_graph{...}`, and the answers are taken with `game.history_take()`
-- (`graph.poll`). An answer is drawn only if it carries the window's latest request number, the current history revision and the window's
-- current column budget (rules 42, 43, 43a); otherwise it is dropped and the window keeps what it shows.
local ui = require("lib.ui")
local windows = require("lib.window")
local draw = require("lib.draw")
local theme = require("lib.theme")
local button = require("lib.button")
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local list = require("lib.list")
local chart = require("lib.graph_chart")
local place_names = require("lib.place_names")
local resize_corner = require("lib.resize_corner")

local P = ui.P
local graph = {}

-- The widths of the three columns (rule 27) and the strip a collapsed set leaves.
local COLUMN_WIDTHS = {70, 86, 112}
local STRIP = 13
local HEADING = 15
local CAPTION = 17
local CHART_TOP = 29
local STAGGER = 16

local RANGES = {{key = "1y", label = "1 year"}, {key = "3y", label = "3 years"}, {key = "10y", label = "10 years"}, {key = "all", label = "all"}}
local PLACE_GRAPHS = {{key = "people", label = "people"}, {key = "births_deaths", label = "births and deaths"}, {key = "food_days", label = "food days"}}
local POP_GRAPHS = PLACE_GRAPHS
local CAPTIONS = {month = "a column a month", season = "a column a season", year = "a column a year", decade = "a column a decade",
  ["quarter century"] = "a column a quarter century", century = "a column a century"}

local env = {view = nil, revision = 0, world = nil, serial = 0, subjects = nil, changed = nil}
local open = {}
local Instance = {}
Instance.__index = Instance

local GOOD_NAMES = {peas_beans = "peas and beans"}

local function good_name(id)
  local entry = glossary ~= nil and glossary.entry(id) or nil
  if entry ~= nil and entry.title ~= nil then return entry.title end
  return GOOD_NAMES[id] or (id:gsub("_", " "))
end

local function title_of(key)
  local entry = glossary ~= nil and glossary.entry(key) or nil
  if entry ~= nil and entry.title ~= nil then return entry.title end
  return key
end

-- The title of a graph (rule 23).
function graph.graph_title(key)
  if key == "people" then return "people" end
  if key == "births_deaths" then return "births and deaths" end
  if key == "food_days" then return "food days" end
  if key == "made" then return "made, by value" end
  if key == "used" then return "used by pops, by value" end
  local good = key:match("^price:(.+)$")
  if good ~= nil then return "price: " .. good_name(good) end
  return key
end

-- A pop's label (rule 23a): its type, with what tells it apart from same-type pops at its place.
function graph.pop_label(pop)
  local label = title_of(pop.kind)
  if pop.adds == "religion" then label = label .. " (" .. title_of(pop.religion) .. ")"
  elseif pop.adds == "culture" then label = label .. " (" .. title_of(pop.culture) .. ")"
  elseif pop.adds == "both" then label = label .. " (" .. title_of(pop.culture) .. ", " .. title_of(pop.religion) .. ")" end
  if (pop.ordinal or 0) > 0 then label = label .. " #" .. pop.ordinal end
  return label
end

local function find_pop(id)
  if env.subjects == nil then return nil end
  for place, pops in pairs(env.subjects.pops or {}) do
    for _, pop in ipairs(pops) do
      if pop.id == id then return pop, place end
    end
  end
  return nil
end

function Instance:subject_title()
  local subject = self.subject
  if subject == nil then return nil end
  if subject.kind == "global" then return "global" end
  if subject.kind == "tag" then return subject.id end
  if subject.kind == "place" then return place_names.name_of(subject.id) end
  -- The latest month's subjects list the pop if it is in that month; otherwise its own latest record (the answer's) names it (rules 23, 23a).
  local pop, place = find_pop(subject.id)
  if pop == nil and self.known_pop ~= nil then pop, place = self.known_pop.pop, self.known_pop.place end
  if pop == nil then return "pop" end
  return graph.pop_label(pop) .. " at " .. place_names.name_of(place)
end

function Instance:title()
  if self.subject == nil or self.graph == nil then return "graphs" end
  return self:subject_title() .. " · " .. graph.graph_title(self.graph)
end

local function series_name(self, key)
  if key == "other" then return "other" end
  if self.graph ~= nil and (self.graph:match("^price:") or self.graph == "made" or self.graph == "used") then return good_name(key) end
  return key
end

-- The colour of series `index` (rule 38): fixed by its place in the graph, never re-assigned; `other` is `dim`; births and deaths keep the
-- colours of the number and the over-limit figure.
local function colour_of(index, key)
  if key == "other" then return theme.colour("dim") end
  if key == "births" then return theme.colour("number") end
  if key == "deaths" then return theme.colour("over_limit") end
  return theme.colour("series_" .. math.min(index, 6))
end

-- ---------------------------------------------------------------------------------------------------------------------------------------
-- Geometry
-- ---------------------------------------------------------------------------------------------------------------------------------------

-- Each column is shown or collapsed on its own (rules 27, 28). A collapsed column leaves a strip `STRIP` wide; a window grows by a shown
-- column's width less its strip, so with all three collapsed it is the least size plus nothing.
local function extra_width(self)
  local extra = 0
  for column = 1, 3 do
    if self.shown[column] then extra = extra + COLUMN_WIDTHS[column] - STRIP end
  end
  return extra * P
end

-- The width of one column's slot, its divider included, and of all three.
local function slot_width(self, column) return (self.shown[column] and COLUMN_WIDTHS[column] or STRIP) + 1 end

local function base_size()
  local view = env.view
  return math.max(300 * P, ui.snap(view.width * 5 / 8)), math.max(120 * P, ui.snap(view.height / 2))
end

function Instance:chart_x()
  return slot_width(self, 1) + slot_width(self, 2) + slot_width(self, 3) + 1
end

function Instance:content_size()
  return self.window.content_width // P, self.window.content_height // P
end

function Instance:chart_width()
  local width = self:content_size()
  return math.max(0, width - self:chart_x())
end

-- The series that stand side by side in a column of this graph (births and deaths), which the budget and the legend leave room for.
function Instance:per() return self.graph == "births_deaths" and 2 or 1 end

function Instance:budget() return chart.budget(self:chart_width(), self:per()) end

local function window_at(x, y)
  for _, other in ipairs(open) do
    if other.window ~= nil and not other.window.closed then
      local ox, oy = other.window:position()
      if ox == x and oy == y then return true end
    end
  end
  return false
end

-- ---------------------------------------------------------------------------------------------------------------------------------------
-- Requests
-- ---------------------------------------------------------------------------------------------------------------------------------------

function Instance:ask()
  if self.subject == nil or self.graph == nil or self.window == nil or self.window.closed then return end
  env.serial = env.serial + 1
  self.serial = env.serial
  self.asked_budget = self:budget()
  game.history_graph{subject = self.subject, graph = self.graph, range = self.range, budget = self.asked_budget, request = self.serial}
end

-- An answer is drawn only if it is for this window's latest request, the current history revision and the current budget (rules 43, 43a).
function Instance:receive(answer)
  if answer.request ~= self.serial or answer.revision ~= env.revision or answer.budget ~= self:budget() then return false end
  self.answer = answer
  if self.subject ~= nil and self.subject.kind == "pop" and answer.pop ~= nil then
    -- The answer names the pop by its own latest record, with what its label adds and its number as the picker would give them there.
    self.known_pop = {pop = {id = self.subject.id, kind = answer.pop.kind, culture = answer.pop.culture, religion = answer.pop.religion,
      adds = answer.pop.adds or "none", ordinal = answer.pop.ordinal or 0}, place = answer.pop.place}
    if self:is_open() then self.window:set_title(self:title()) end
  end
  self:draw()
  return true
end

-- ---------------------------------------------------------------------------------------------------------------------------------------
-- Drawing
-- ---------------------------------------------------------------------------------------------------------------------------------------

function Instance:clear_content()
  if self.view_group ~= nil then self.view_group:destroy(); self.view_group = nil end
  for _, widget in ipairs(self.buttons or {}) do widget:destroy() end
  self.buttons = {}
  for _, one in ipairs(self.lists or {}) do one:destroy() end
  self.lists = {}
  if self.plot_key ~= nil then
    render.set_hit(self.plot_key, nil, self.window.content)
    tooltip.detach(self.plot_key)
    pointer.unregister(self.plot_key)
    self.plot_key = nil
  end
end

-- The answer that is drawn: the current one, unless its columns do not fit the budget (rule 43a).
function Instance:shown_answer()
  local answer = self.answer
  if answer == nil then return nil end
  if answer.columns ~= nil and #answer.columns > self:budget() then return nil end
  return answer
end

function Instance:spec()
  local cw = self:chart_width()
  local _, height = self:content_size()
  return {width = cw, height = math.max(1, height - CHART_TOP), per = self:per(), colour_of = colour_of, name_of = function(key) return series_name(self, key) end,
    scale = self.graph ~= nil and self.graph:match("^price:") and 0.001 or 1, dim = theme.colour("dim"), track = theme.colour("track")}
end

-- Draws the caption, the plot and its legend, and sets the plot's tooltip. The columns and the buttons are laid out by `build`.
function Instance:draw()
  if self.window == nil or self.window.closed then return end
  if self.chart_view ~= nil then self.chart_view:destroy() end
  local group = self.window.content
  local view = draw.view()
  self.chart_view = view
  local cx, cy = self:chart_x(), CHART_TOP
  local answer = self:shown_answer()
  local spec = self:spec()
  local layout = chart.layout(answer, spec)
  self.layout = layout
  local dim = theme.colour("dim")
  if self.subject == nil or self.graph == nil then
    draw.text(view, group, cx + layout.plot.x + 4, cy + layout.plot.y + 4, "choose a place and a graph", dim, "small")
    return
  end
  if answer ~= nil and answer.step ~= nil then
    draw.text(view, group, cx + chart.GUTTER + 2, CAPTION, CAPTIONS[answer.step] or "", dim, "small")
  end
  if layout.message ~= nil then
    draw.text(view, group, cx + layout.plot.x + 4, cy + layout.plot.y + 4, layout.message, dim, "small")
    return
  end
  for _, rect in ipairs(layout.rects) do draw.rect(view, group, cx + rect.x, cy + rect.y, rect.w, rect.h, rect.colour) end
  for _, label in ipairs(layout.labels) do draw.text(view, group, cx + label.x, cy + label.y, label.text, label.colour, "small", label.align) end
  for _, row in ipairs(layout.legend) do
    draw.rect(view, group, cx + row.x, cy + row.y + 1, 5, 5, row.swatch)
    draw.text(view, group, cx + row.x + 8, cy + row.y, draw.clip(row.text, chart.LEGEND - 8, "small"), theme.colour("text"), "small")
  end
end

function Instance:set_plot_hit()
  local plot = self.layout.plot
  local cx = self:chart_x()
  if self.plot_key == nil then
    self.plot_key = ui.key("graph_plot")
    local widget = {on_event = function() end, on_hover = function() end, on_overflow = function() end, functional = function() return false end}
    pointer.register(self.plot_key, widget, self.window)
    tooltip.attach(self.plot_key, function(x, y)
      if self.window == nil or self.window.closed or self.layout == nil then return nil end
      local answer = self:shown_answer()
      if answer == nil or answer.columns == nil then return nil end
      local ox = self.window:content_origin()
      local index = chart.column_at(self.layout, (x - ox) / P - self:chart_x())
      if index == nil then return nil end
      return chart.tooltip(answer, index, self:spec()), tostring(index)
    end)
  end
  render.set_hit(self.plot_key, {{x = (cx + plot.x) * P, y = (CHART_TOP + plot.y) * P, width = plot.w * P, height = plot.h * P}}, self.window.content)
end

-- ---------------------------------------------------------------------------------------------------------------------------------------
-- Columns
-- ---------------------------------------------------------------------------------------------------------------------------------------

local function lower(text) return text:lower() end

function Instance:tags()
  local result = {}
  for _, tag in ipairs(env.subjects ~= nil and env.subjects.tags or {"global"}) do result[#result + 1] = tag end
  return result
end

-- The places of the chosen tag, by shown name (rule 25).
function Instance:place_rows()
  local rows = {}
  local ids = env.subjects ~= nil and env.subjects.places[self.tag] or {}
  for _, id in ipairs(ids) do rows[#rows + 1] = {id = id, name = place_names.name_of(id)} end
  table.sort(rows, function(a, b)
    if lower(a.name) ~= lower(b.name) then return lower(a.name) < lower(b.name) end
    return a.id < b.id
  end)
  return rows
end

-- The graphs of the chosen subject (rule 25): a place's own, then each pop's under a dim heading with its type.
function Instance:graph_rows()
  local rows = {}
  local pick = self.pick
  if pick == nil then return rows end
  local function add(subject, key, label) rows[#rows + 1] = {subject = subject, graph = key, label = label} end
  for _, entry in ipairs(PLACE_GRAPHS) do add(pick, entry.key, entry.label) end
  for _, good in ipairs(env.subjects ~= nil and env.subjects.goods or {}) do add(pick, "price:" .. good, "price: " .. good_name(good)) end
  add(pick, "made", "made, by value")
  add(pick, "used", "used by pops, by value")
  if pick.kind == "place" then
    for _, pop in ipairs(env.subjects ~= nil and env.subjects.pops[pick.id] or {}) do
      rows[#rows + 1] = {heading = graph.pop_label(pop)}
      for _, entry in ipairs(POP_GRAPHS) do add({kind = "pop", id = pop.id}, entry.key, entry.label) end
    end
  end
  return rows
end

local function same_subject(a, b)
  return a ~= nil and b ~= nil and a.kind == b.kind and a.id == b.id
end

function Instance:choose_tag(tag)
  self.following = false
  self.tag = tag
  self.pick = {kind = tag == "global" and "global" or "tag", id = tag}
  self:build()
end

function Instance:choose_place(row)
  self.following = false
  if row == nil then self.pick = {kind = self.tag == "global" and "global" or "tag", id = self.tag}
  else self.pick = {kind = "place", id = row.id} end
  self:build()
end

function Instance:choose_graph(row)
  self.subject, self.graph = row.subject, row.graph
  self.known_pop = nil
  self.window:set_title(self:title())
  self.answer = nil
  self:ask()
  self:build()
end

local function clip_to(text, width) return draw.clip(text, width, nil) end

-- One column's list in the window: `x` and `width` in UI pixels.
function Instance:make_list(x, width, rows, items, on_select, selected)
  local height = self.window.content_height // P - HEADING
  local count = math.max(1, height // 13)
  local one = list.new{window = self.window, x = x * P, y = HEADING * P, width = width * P, rows = count, items = items, selectable = true, on_select = on_select}
  if selected ~= nil then one:select(selected); one:scroll_to(selected) end
  self.lists[#self.lists + 1] = one
  return one
end

function Instance:build_columns()
  local win = self.window
  local height = win.content_height // P
  local dim = theme.colour("dim")
  local tags = self:tags()
  local places = self:place_rows()
  local graphs = self:graph_rows()
  local headings = {"tags", "places", "graphs"}
  -- What column `column` lists, the row chosen in it and what choosing a row does.
  local function contents(column)
    local items, chosen, select = {}, nil, nil
    if column == 1 then
      for index, tag in ipairs(tags) do
        items[index] = clip_to(tag, COLUMN_WIDTHS[1] - 8)
        if tag == self.tag then chosen = index end
      end
      select = function(index) if tags[index] ~= nil then self:choose_tag(tags[index]) end end
    elseif column == 2 then
      items[1] = clip_to(self.tag == "global" and "global" or (self.tag .. " (all)"), COLUMN_WIDTHS[2] - 8)
      if same_subject(self.pick, {kind = self.tag == "global" and "global" or "tag", id = self.tag}) then chosen = 1 end
      for index, row in ipairs(places) do
        items[index + 1] = clip_to(row.name, COLUMN_WIDTHS[2] - 8)
        if self.pick ~= nil and self.pick.kind == "place" and self.pick.id == row.id then chosen = index + 1 end
      end
      select = function(index)
        if index == 1 then self:choose_place(nil) elseif places[index - 1] ~= nil then self:choose_place(places[index - 1]) end
      end
    else
      for index, row in ipairs(graphs) do
        if row.heading ~= nil then
          items[index] = {cells = {{text = clip_to(row.heading, COLUMN_WIDTHS[3] - 8), x = 1, align = "left", colour = dim}}}
        else
          items[index] = clip_to(row.label, COLUMN_WIDTHS[3] - 8)
          if same_subject(row.subject, self.subject) and row.graph == self.graph then chosen = index end
        end
      end
      select = function(index)
        local row = graphs[index]
        if row ~= nil and row.heading == nil then self:choose_graph(row) end
      end
    end
    return items, chosen, select
  end
  local x = 0
  for column = 1, 3 do
    local width = COLUMN_WIDTHS[column]
    if self.shown[column] then
      draw.text(self.chart_view_static, win.content, x + 3, 2, headings[column], dim, "small")
      self.buttons[#self.buttons + 1] = button.new{x = (x + width - 10) * P, y = 0, text = "<", kind = win.kind, group = win.content, owner = win,
        tooltip = "hide", on_click = function() self:set_column(column, false) end}
      local items, chosen, select = contents(column)
      self:make_list(x, width, nil, items, select, chosen)
    else
      -- A collapsed column leaves its strip, with the button that shows it again.
      self.buttons[#self.buttons + 1] = button.new{x = x * P, y = 0, text = ">", kind = win.kind, group = win.content, owner = win,
        tooltip = "show " .. headings[column], on_click = function() self:set_column(column, true) end}
    end
    x = x + slot_width(self, column)
    draw.rect(self.chart_view_static, win.content, x - 1, 0, 1, height, win.colours.outline)
  end
end

-- ---------------------------------------------------------------------------------------------------------------------------------------
-- The window
-- ---------------------------------------------------------------------------------------------------------------------------------------

-- Shows or hides one column; the window grows or shrinks by its width less its strip, kept on screen (rule 28).
function Instance:set_column(column, shown)
  if self.shown[column] == shown then return end
  local delta = (COLUMN_WIDTHS[column] - STRIP) * P
  self.shown[column] = shown
  local w, h = self.window:size()
  self.window:set_outer_size(math.max(300 * P + extra_width(self), w + (shown and delta or -delta)), h)
  self:fit()
end

-- Shows or hides all three.
function Instance:set_columns(shown)
  for column = 1, 3 do self:set_column(column, shown) end
end

-- Lays everything out again for the window's size.
function Instance:build()
  if self.window == nil or self.window.closed then return end
  self:clear_content()
  if self.chart_view ~= nil then self.chart_view:destroy(); self.chart_view = nil end
  if self.chart_view_static ~= nil then self.chart_view_static:destroy() end
  self.chart_view_static = draw.view()
  local win = self.window
  local width = win.content_width // P
  self:build_columns()
  -- The range buttons at the toolbar's right.
  local right = width - 1
  for index = #RANGES, 1, -1 do
    local entry = RANGES[index]
    local bw = button.measure(entry.label) // P
    local range = button.new{x = (right - bw) * P, y = 0, text = entry.label, kind = win.kind, group = win.content, owner = win,
      on_click = function() self:set_range(entry.key) end}
    range:set_selected(self.range == entry.key)
    self.buttons[#self.buttons + 1] = range
    right = right - bw - 3
  end
  -- The corner stays above what the build made.
  if self.corner ~= nil then win:detach(self.corner); win:attach(self.corner) end
  local budget = self:budget()
  if self.subject ~= nil and self.graph ~= nil and self.asked_budget ~= budget then self:ask() end
  self:draw()
  self:set_plot_hit()
end

function Instance:set_range(key)
  if self.range == key then return end
  self.range = key
  self:ask()
  self:build()
end

-- Keeps the window within its least size and the view, and lays it out again.
function Instance:fit()
  if self.window == nil or self.window.closed then return end
  local view = env.view
  local min_width = 300 * P + extra_width(self)
  local w, h = self.window:size()
  w = math.max(min_width, math.min(w, view.width))
  h = math.max(120 * P, math.min(h, view.height))
  self.window:set_outer_size(w, h)
  local x, y = self.window:position()
  if view.width < min_width or view.height < 120 * P then
    x, y = view.margin or 8, view.margin or 8
  else
    x = math.max(0, math.min(x, view.width - w))
    y = math.max(0, math.min(y, view.height - h))
  end
  self.window:move_to(x, y)
  self:build()
end

function Instance:is_open() return self.window ~= nil and not self.window.closed end

local function forget(instance)
  for index, candidate in ipairs(open) do
    if candidate == instance then table.remove(open, index); break end
  end
end

-- ---------------------------------------------------------------------------------------------------------------------------------------
-- The manager
-- ---------------------------------------------------------------------------------------------------------------------------------------

-- Sets what the windows need: the view (its width and height, read live) and a function told when the number of open windows changes.
function graph.init(spec)
  env.view = spec.view
  env.changed = spec.changed
end

function graph.windows() return open end

-- Opens a graph window and returns it. `spec.subject` (`{kind, id}`) and `spec.graph` name what it shows (both or neither); `spec.columns` shows
-- the columns. A window that shows the same subject and graph already comes to the top instead (rule 30).
function graph.open(spec)
  if env.view == nil then error("graph.open: init was not called", 2) end
  if spec.subject ~= nil and spec.graph ~= nil and not spec.columns then
    for _, other in ipairs(open) do
      if other:is_open() and same_subject(other.subject, spec.subject) and other.graph == spec.graph then
        other.window:raise()
        return other
      end
    end
  end
  local self = setmetatable({subject = spec.subject, graph = spec.graph, range = "3y", following = true, shown = {spec.columns == true, spec.columns == true, spec.columns == true}, buttons = {}, lists = {}, tag = "global"}, Instance)
  if env.subjects == nil and game.history_subjects ~= nil then env.subjects = game.history_subjects() end
  if self.subject ~= nil then
    self.pick = self.subject
    if self.subject.kind == "pop" then
      local _, place = find_pop(self.subject.id)
      if place ~= nil then self.pick = {kind = "place", id = place} end
    end
  end
  local w, h = base_size()
  w = w + extra_width(self)
  local x = ui.snap((env.view.width - w) / 2)
  local y = ui.snap((env.view.height - h) / 2)
  while window_at(x, y) do x, y = x + STAGGER * P, y + STAGGER * P end
  local created
  created = windows.new{title = self:title(), kind = "window", outer_width = w, outer_height = h, x = x, y = y, on_close = function()
    -- The window's groups, hits and pointer widgets went with it; the tooltip keeps its own table of sources, so the plot's goes here, and the
    -- instance lets go of what it drew and of its answer.
    if self.plot_key ~= nil then tooltip.detach(self.plot_key); self.plot_key = nil end
    self.window, self.answer, self.layout, self.corner = nil, nil, nil, nil
    self.buttons, self.lists, self.chart_view, self.chart_view_static, self.subjects = {}, {}, nil, nil, nil
    forget(self)
    if env.changed ~= nil then env.changed(#open) end
  end}
  self.window = created
  self.corner = resize_corner.new{window = created, view = env.view, prefix = "graph_resize", min_width = function() return 300 * P + extra_width(self) end,
    min_height = 120 * P, on_change = function() self:build() end}
  open[#open + 1] = self
  self:fit()
  if env.changed ~= nil then env.changed(#open) end
  return self
end

-- Takes the answers the host has ready and gives each to its window.
function graph.poll()
  if #open == 0 or game.history_take == nil then return end
  for _, answer in ipairs(game.history_take()) do
    for _, one in ipairs(open) do
      if one.serial == answer.request then one:receive(answer); break end
    end
  end
end

-- Follows `game_time`: a new history revision or world asks every window again (rule 43); a load clears what they show until the new answer.
function graph.refresh(time)
  local revision = time.history_revision or 0
  local world = time.world or 0
  local loaded = env.world ~= nil and env.world ~= world
  env.world = world
  if revision == env.revision and not loaded then return end
  env.revision = revision
  if game.history_subjects ~= nil then env.subjects = game.history_subjects() end
  for _, one in ipairs(open) do
    if loaded then
      -- A different history: the answer, the pop it named and the place the picker followed it to belong to the world that is gone; the
      -- selected id is resolved afresh from the new subjects and the new answer (rules 23, 43).
      one.answer = nil
      one.known_pop = nil
      if one.following then one.pick = {kind = one.tag == "global" and "global" or "tag", id = one.tag} end
    end
    -- A pop keeps its id but its latest record may name another place or label: the title and the picker's place follow it (rules 23, 23a).
    -- The picker follows the displayed pop to its place only while the player has not chosen a tag or place of their own.
    if one.following and one.subject ~= nil and one.subject.kind == "pop" then
      local _, place = find_pop(one.subject.id)
      if place == nil and one.known_pop ~= nil then place = one.known_pop.place end
      if place ~= nil then one.pick = {kind = "place", id = place} end
    end
    if one:is_open() then one.window:set_title(one:title()) end
    one:ask()
    one:build()
  end
end

function graph.fit() for _, one in ipairs({table.unpack(open)}) do one:fit() end end

-- The place names show differently (CK-423): titles and rows are made again.
function graph.relabel()
  for _, one in ipairs(open) do
    if one:is_open() then one.window:set_title(one:title()); one:build() end
  end
end

function graph.close_all() for _, one in ipairs({table.unpack(open)}) do if one:is_open() then one.window:close() end end end

-- For tests: forgets the manager's state.
function graph.reset()
  graph.close_all()
  open = {}
  env = {view = env.view, revision = 0, world = nil, serial = 0, subjects = nil, changed = env.changed}
end

-- What all three columns add to a window's width over all three collapsed, in UI pixels (rule 28).
graph.ALL_EXTRA = COLUMN_WIDTHS[1] + COLUMN_WIDTHS[2] + COLUMN_WIDTHS[3] - 3 * STRIP
return graph
