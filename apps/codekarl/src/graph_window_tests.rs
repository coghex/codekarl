//! CK-441 (history slice B): the graph windows, their chart layout and the requests they make, in the UI rig and without a window.
//!
//! Each case is a Lua script run against the real UI libraries with the three history calls (`game.history_subjects`, `game.history_graph`,
//! `game.history_take`) replaced by fixtures the script owns, so every answer arrives exactly when the case delivers it: nothing sleeps.
//! A script asserts in Lua and records `ok` or the error; the case reads the records.

use crate::readout_tests::Readout;

/// What every script starts with: the libraries, three places and two same-type pops at one of them, and the fixture host.
const PRELUDE: &str = r"
local windows = require('lib.window')
windows.update_viewport(1280, 800)
local graph = require('lib.graph_window')
local chart = require('lib.graph_chart')
local P = require('lib.ui').P
local view = {width = 1280, height = 800}
graph.init{view = view}
local requests, pending = {}, {}
game.places = function() return {{id = 'cong', name = 'Cong'}, {id = 'armagh', name = 'Armagh'}} end
game.history_subjects = function()
  return {revision = 1, tags = {'global'}, places = {global = {'cong', 'armagh'}}, goods = {'oats'}, pops = {
    cong = {
      {id = '0000000000000001', kind = 'peasant', culture = 'irish', religion = 'pagan', adds = 'religion', ordinal = 0},
      {id = '0000000000000002', kind = 'peasant', culture = 'irish', religion = 'christian', adds = 'religion', ordinal = 0},
    },
    armagh = {
      {id = '0000000000000003', kind = 'monk', culture = 'irish', religion = 'christian', adds = 'both', ordinal = 2},
    },
  }}
end
game.history_graph = function(request) requests[#requests + 1] = request end
game.history_take = function() local out = pending; pending = {}; return out end
local function columns(count, values)
  local result = {}
  for index = 1, count do
    result[index] = {from = {year = 770, month = 1}, to = {year = 770, month = 1}, months = 1, record = true, values = values}
  end
  return result
end
local function answer(request, revision, budget, count)
  return {request = request.request, revision = revision, budget = budget, step = 'month', horizon = {year = 770, month = 12},
    stacked = true, rate = false, series = {'workers', 'dependents'}, columns = columns(count or 3, {4.0, 2.0})}
end
local function run(body)
  local ok, err = pcall(body)
  window.record(ok and 'ok' or ('error ' .. tostring(err)))
end
";

fn run(body: &str) -> Vec<String> {
    let readout = Readout::script(&format!("{PRELUDE}\n{body}"));
    readout
        .entries()
        .iter()
        .filter_map(|entry| entry.strip_prefix("record ").map(str::to_owned))
        .filter(|record| record != "mark")
        .collect()
}

fn ok(body: &str) {
    let records = run(body);
    assert_eq!(records, ["ok"], "{records:?}");
}

#[test]
fn a_graph_opens_centred_with_its_title_collapsed_columns_and_one_request() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local first = graph.open{subject = {kind = 'place', id = 'cong'}, graph = 'people'}
  assert(first.window.title == 'Cong · people', first.window.title)
  assert(not (first.shown[1] or first.shown[2] or first.shown[3]) and #first.lists == 0, 'an icon opens all three columns collapsed')
  local w, h = first.window:size()
  assert(w == 800 and h == 400, 'five eighths by one half: ' .. w .. ' ' .. h)
  local x, y = first.window:position()
  assert(x == 240 and y == 200, 'centred: ' .. x .. ' ' .. y)
  assert(#requests == 1, 'requests: ' .. #requests)
  assert(requests[1].range == '3y' and requests[1].graph == 'people', 'one request on three years')
  assert(requests[1].subject.kind == 'place' and requests[1].subject.id == 'cong')
  assert(requests[1].budget == chart.budget(first:chart_width()) , requests[1].budget)
  -- The same subject and graph comes to the top instead of opening again (rule 30).
  local second = graph.open{subject = {kind = 'place', id = 'armagh'}, graph = 'people'}
  local again = graph.open{subject = {kind = 'place', id = 'cong'}, graph = 'people'}
  assert(again == first and #graph.windows() == 2 and windows.top() == first.window, 'the open window is raised')
  -- A window opened over another at the centre spot opens 16 UI pixels down and right of it (rule 22).
  local sx, sy = second.window:position()
  assert(sx == 240 + 16 * P and sy == 200 + 16 * P, 'staggered: ' .. sx .. ' ' .. sy)
end)
");
}

#[test]
fn the_time_bars_button_opens_with_the_columns_shown_and_nothing_chosen() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{columns = true}
  assert(one.shown[1] and one.shown[2] and one.shown[3] and one.window.title == 'graphs' and #requests == 0, 'nothing is asked before a choice')
  local w = one.window:size()
  assert(w == math.min(800 + graph.ALL_EXTRA * P, 1280), 'the columns widen the window, kept on screen: ' .. w)
  assert(#one.lists == 3, 'three columns')
  -- Choosing a graph in the third column asks for it.
  local rows = one:graph_rows()
  assert(#rows == 0, 'no subject yet, so no graphs')
  one:choose_tag('global')
  one:choose_place({id = 'cong', name = 'Cong'})
  rows = one:graph_rows()
  assert(rows[1].graph == 'people' and rows[2].graph == 'births_deaths' and rows[3].graph == 'food_days', 'the place graphs first')
  assert(rows[4].graph == 'price:oats' and rows[5].graph == 'made' and rows[6].graph == 'used')
  assert(rows[7].heading == 'peasant (pagan)' and rows[8].subject.kind == 'pop' and rows[8].subject.id == '0000000000000001', 'pops by id')
  assert(rows[10].heading == nil and rows[11].heading == 'peasant (christian)', 'each pop its own heading')
  one:choose_graph(rows[2])
  assert(#requests == 1 and requests[1].graph == 'births_deaths' and requests[1].subject.id == 'cong')
  assert(one.window.title == 'Cong · births and deaths')
end)
");
}

#[test]
fn answers_are_drawn_only_for_the_latest_request_revision_and_budget() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{subject = {kind = 'place', id = 'cong'}, graph = 'people'}
  local first = requests[1]
  -- Another request number is dropped, the window's own is drawn.
  assert(not one:receive(answer({request = first.request + 99}, 5, one:budget())) and one.answer == nil)
  pending[1] = answer(first, 5, one:budget())
  graph.poll()
  assert(one.answer ~= nil and one.answer.request == first.request, 'the answer came through the queue')
  -- A range changed twice gets its first answer last: it is dropped.
  one:set_range('1y')
  one:set_range('10y')
  assert(#requests == 3 and requests[2].range == '1y' and requests[3].range == '10y')
  local kept = one.answer
  assert(not one:receive(answer(requests[2], 5, one:budget())) and one.answer == kept, 'the first answer arrives last and is dropped')
  assert(one:receive(answer(requests[3], 5, one:budget())) and one.answer ~= kept)
  -- A month recorded (a new history revision): every window asks again and gains it; an answer of the old revision is dropped.
  graph.refresh{history_revision = 6, world = 0}
  assert(#requests == 4 and requests[4].request ~= requests[3].request)
  assert(not one:receive(answer(requests[4], 5, one:budget())), 'computed from the old history')
  assert(one:receive(answer(requests[4], 6, one:budget(), 4)) and #one.answer.columns == 4)
  -- A load while an answer is outstanding: the window shows nothing, asks again, and the old answer is dropped.
  graph.refresh{history_revision = 7, world = 1}
  assert(one.answer == nil and #requests == 5, 'a load clears what is shown')
  assert(not one:receive(answer(requests[4], 6, one:budget())), 'the old history is dropped')
  assert(one:receive(answer(requests[5], 7, one:budget())))
end)
");
}

#[test]
fn a_resize_that_lowers_the_budget_asks_again_keeps_a_fitting_chart_and_drops_one_that_does_not() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{subject = {kind = 'place', id = 'cong'}, graph = 'people'}
  assert(one:receive(answer(requests[1], 5, one:budget(), 225)))
  assert(one:shown_answer() ~= nil, 'a chart that fits is drawn')
  local before = #requests
  one.window:set_outer_size(300 * P, 200)
  one:fit()
  local budget = one:budget()
  assert(budget < 225 and budget == chart.budget(one:chart_width()), 'the plot is narrower: ' .. budget)
  assert(#requests == before + 1 and requests[#requests].budget == budget, 'a new request carries the new budget')
  assert(one:shown_answer() == nil, 'a chart with more columns than the plot has pixels is not drawn')
  assert(not one:receive(answer(requests[before], 5, one:budget() + 30, 10)), 'an answer computed for the old budget is dropped')
  assert(one:receive(answer(requests[#requests], 5, budget, 10)) and one:shown_answer() ~= nil)
  -- A chart that fits the lower budget stays drawn while the new answer is awaited.
  assert(one:receive(answer(requests[#requests], 5, budget, 10)))
  one.window:set_outer_size(350 * P, 300)
  assert(one:shown_answer() ~= nil, 'ten columns fit any budget')
  -- Showing the columns widens the window by their width and asks for the narrower plot.
  local w = one.window:size()
  local asked = #requests
  one:set_columns(true)
  local wider = one.window:size()
  assert(wider - w == graph.ALL_EXTRA * P, 'widened by ' .. (wider - w))
  assert(#requests > asked - 1 and requests[#requests].budget == one:budget(), 'the budget follows the plot')
  one:set_columns(false)
  assert(one.window:size() == w, 'hiding narrows it back')
end)
");
}

#[test]
fn escape_closes_the_top_graph_window_and_the_corner_resizes_to_the_least_size() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{subject = {kind = 'place', id = 'cong'}, graph = 'people'}
  local two = graph.open{subject = {kind = 'place', id = 'armagh'}, graph = 'people'}
  assert(windows.top() == two.window)
  windows.close_request('key')
  assert(#graph.windows() == 1 and not two:is_open() and one:is_open(), 'Escape closes the top window')
  -- Dragged far in, the corner stops at 300 by 120 UI pixels; with the columns shown the least width grows by theirs.
  local x, y = one.window:position()
  local w, h = one.window:size()
  one.corner:on_event({kind = 'down', x = x + w, y = y + h}, true)
  one.corner:on_event({kind = 'move', x = x + w - 2000, y = y + h - 2000}, true)
  one.corner:on_event({kind = 'up', x = x + w - 2000, y = y + h - 2000}, true)
  w, h = one.window:size()
  assert(w == 300 * P and h == 120 * P, 'least size: ' .. w .. ' ' .. h)
  one:set_columns(true)
  w = one.window:size()
  assert(w == 300 * P + graph.ALL_EXTRA * P, 'the least width keeps the plot as wide: ' .. w)
end)
");
}

#[test]
fn same_type_pops_get_distinct_labels_and_each_window_asks_for_its_own_pop_by_id() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local pagan = graph.open{subject = {kind = 'pop', id = '0000000000000001'}, graph = 'people'}
  local christian = graph.open{subject = {kind = 'pop', id = '0000000000000002'}, graph = 'people'}
  assert(pagan.window.title == 'peasant (pagan) at Cong · people', pagan.window.title)
  assert(christian.window.title == 'peasant (christian) at Cong · people', christian.window.title)
  assert(requests[1].subject.id == '0000000000000001' and requests[2].subject.id == '0000000000000002', 'each asks for its own id')
  assert(graph.open{subject = {kind = 'pop', id = '0000000000000002'}, graph = 'people'} == christian, 'the check uses ids')
  assert(graph.pop_label({kind = 'monk', culture = 'irish', religion = 'christian', adds = 'both', ordinal = 2}) == 'monk (irish, christian) #2')
  assert(graph.pop_label({kind = 'peasant', culture = 'irish', religion = 'pagan', adds = 'none', ordinal = 0}) == 'peasant')
  assert(#graph.windows() == 2)
end)
");
}

#[test]
fn the_chart_draws_stacked_columns_axis_lines_gaps_and_the_legend_by_the_rules() {
    ok(r"
run(function()
  local spec = {width = 400, height = 150, dim = '#dim', track = '#track', scale = 1,
    colour_of = function(index) return '#c' .. index end, name_of = function(key) return key end}
  local plot_w, legend = chart.plot_width(400)
  assert(legend and plot_w == 400 - 32 - 72 - 6 - 3, 'the legend takes 72 and its gap: ' .. plot_w)
  local _, narrow = chart.plot_width(240)
  assert(not narrow, 'a plot narrower than 160 with the legend leaves the legend out')
  assert(chart.budget(240) == 240 - 32 - 3 and chart.budget(2000) == 250, 'the budget is the smaller of 250 and the pixels')
  local columns = {
    {from = {year = 770, month = 1}, to = {year = 770, month = 1}, months = 1, record = true, values = {4.0, 2.0}},
    {from = {year = 770, month = 2}, to = {year = 770, month = 2}, months = 0, record = false},
    {from = {year = 770, month = 3}, to = {year = 770, month = 3}, months = 1, record = true, values = {0.0, 0.0}},
  }
  local layout = chart.layout({step = 'month', stacked = true, rate = false, series = {'workers', 'dependents'}, columns = columns}, spec)
  assert(layout.maximum == 6, 'a round maximum just above the largest column: ' .. layout.maximum)
  local plot = layout.plot
  assert(plot.h == 150 - 5 - 12)
  local bars, axes = {}, {}
  for _, rect in ipairs(layout.rects) do
    if rect.colour == '#c1' or rect.colour == '#c2' then bars[#bars + 1] = rect end
    if rect.colour == '#dim' and rect.h == 1 and rect.y == plot.y + plot.h then axes[#axes + 1] = rect end
  end
  assert(#bars == 2, 'only the one column with a value has bars: ' .. #bars)
  assert(bars[1].colour == '#c1' and bars[1].h == 89 and bars[2].colour == '#c2' and bars[2].h == 44, 'stacked from the axis: ' .. bars[1].h .. ' ' .. bars[2].h)
  assert(bars[1].y == plot.y + plot.h - 89 and bars[2].y == plot.y + plot.h - 133, 'the second sits on the first')
  assert(bars[1].w == layout.columns[1].w - 1, 'a one pixel gap between columns')
  assert(#axes == 2, 'the axis line is under a recorded column, a measured zero included, and not under a gap: ' .. #axes)
  assert(axes[1].x == layout.columns[1].x and axes[2].x == layout.columns[3].x)
  assert(#layout.legend == 2 and layout.legend[1].text == 'workers' and layout.legend[2].swatch == '#c2')
  assert(chart.column_at(layout, layout.columns[2].x + 1) == 2 and chart.column_at(layout, 1) == nil)
  -- Side by side, not stacked: each series its own bar.
  local pair = chart.layout({step = 'month', stacked = false, rate = true, series = {'births', 'deaths'}, columns = {
    {from = {year = 770, month = 1}, to = {year = 770, month = 1}, months = 1, record = true, values = {10.0, false}, counts = {3, 2}, people = 0.0},
  }}, spec)
  local first, second = nil, nil
  for _, rect in ipairs(pair.rects) do if rect.colour == '#c1' then first = rect elseif rect.colour == '#c2' then second = rect end end
  assert(first ~= nil and second == nil, 'an undefined rate draws nothing for its series')
  -- A message instead of columns.
  local message = chart.layout({message = 'too narrow'}, spec)
  assert(message.message == 'too narrow' and #message.rects == 0)
end)
");
}

#[test]
fn a_columns_tooltip_names_its_period_values_counts_and_the_three_cases() {
    ok(r"
run(function()
  local spec = {name_of = function(key) return key end, scale = 1}
  local rate = {step = 'month', rate = true, series = {'births', 'deaths'}, columns = {
    {from = {year = 770, month = 3}, to = {year = 770, month = 3}, months = 1, record = true, values = {0.0, 12.34}, counts = {0, 3}, people = 412.0},
    {from = {year = 770, month = 4}, to = {year = 770, month = 4}, months = 0, record = false},
    {from = {year = 770, month = 5}, to = {year = 770, month = 5}, months = 2, record = true, values = {false, false}, counts = {0, 2}, people = 0.0},
  }}
  local measured = chart.tooltip(rate, 1, spec)
  assert(measured.title == 'Mar 770' and measured[2] == 'deaths 12.3 a year per 1,000 (3 deaths of 412 people)', measured[2])
  assert(measured[1] == 'births 0 a year per 1,000 (0 births of 412 people)', 'a measured zero says 0: ' .. measured[1])
  assert(chart.tooltip(rate, 2, spec)[1] == 'no record')
  assert(chart.tooltip(rate, 3, spec)[2] == '2 deaths, no people', 'an undefined rate gives the raw count')
  assert(chart.period({from = {year = 769, month = 12}, to = {year = 770, month = 2}}, 'season') == 'Dec 769 to Feb 770')
  assert(chart.period({from = {year = 760, month = 1}, to = {year = 769, month = 12}}, 'decade') == '760 to 769')
  assert(chart.period({from = {year = 770, month = 1}, to = {year = 770, month = 12}}, 'year') == '770')
  local state = {step = 'month', rate = false, series = {'oats'}, columns = {
    {from = {year = 770, month = 3}, to = {year = 770, month = 3}, months = 1, record = true, values = {96500.0}}}}
  assert(chart.tooltip(state, 1, {name_of = function(key) return key end, scale = 0.001})[1] == 'oats 96.5', 'a price in the thousandths shown whole')
end)
");
}

#[test]
fn a_list_hotspot_clicks_through_lights_a_hot_square_and_selects_nothing() {
    ok(r"
run(function()
  local list = require('lib.list')
  local pointer = require('lib.pointer')
  local panel = windows.new{title = 'rows', kind = 'place', outer_width = 400, outer_height = 200}
  local clicked, selected = 0, 0
  local one = list.new{window = panel, x = 0, y = 0, width = 300, rows = 4, selectable = true, on_select = function() selected = selected + 1 end,
    items = {{cells = {{text = '412 people', x = 1, align = 'left'}}, hotspots = {{x = 80, icon = 'graph', on_click = function() clicked = clicked + 1 end}}}, 'plain'}}
  local spot = one.lines[1].hot[1]
  assert(spot ~= nil and one.lines[2].hot == nil, 'the first row has its icon')
  local widget = pointer.widget(spot.key)
  widget:on_hover(true)
  assert(spot.hovered, 'hovering lights the hot square')
  widget:on_event({kind = 'down'}, true)
  widget:on_event({kind = 'up', inside = true}, true)
  assert(clicked == 1 and selected == 0, 'a click opens the graph and chooses no row')
  widget:on_event({kind = 'down'}, true)
  widget:on_event({kind = 'up', inside = false}, true)
  assert(clicked == 1, 'released outside is no click')
  widget:on_hover(false)
  assert(not spot.hovered)
  -- Replacing the items makes the icons again for the new rows, and none for a row without.
  one:set_items({'plain', {cells = {{text = 'x', x = 1, align = 'left'}}, hotspots = {{x = 20, icon = 'graph', on_click = function() end}}}})
  assert(one.lines[1].hot == nil and one.lines[2].hot ~= nil and #one.lines[2].hot == 1)
  one:destroy()
end)
");
}

#[test]
fn the_longest_range_at_the_least_window_draws_both_series_in_every_column() {
    // Review 1: 93 century columns of births and deaths in the 166-pixel plot of the least window are mixed one- and two-pixel cells; a
    // cell narrower than its series draws them over each other, the tallest first, so no positive value is lost.
    ok(r"
run(function()
  local spec = {width = 201, height = 150, dim = '#dim', track = '#track', scale = 1,
    colour_of = function(index) return '#c' .. index end, name_of = function(key) return key end}
  local plot_w = chart.plot_width(201)
  assert(plot_w == 166, 'the least window plot: ' .. plot_w)
  local columns = {}
  for index = 1, 93 do
    columns[index] = {from = {year = 700 + index, month = 1}, to = {year = 700 + index, month = 12}, months = 12, record = true,
      values = {10.0 + index % 7, 20.0 + index % 5}, counts = {3, 4}, people = 400.0}
  end
  local layout = chart.layout({step = 'century', stacked = false, rate = true, series = {'births', 'deaths'}, columns = columns}, spec)
  local widths = {}
  for index, column in ipairs(layout.columns) do widths[column.w] = true end
  assert(widths[1] and widths[2], 'mixed one and two pixel cells')
  for index, column in ipairs(layout.columns) do
    local seen = {}
    for _, rect in ipairs(layout.rects) do
      if (rect.colour == '#c1' or rect.colour == '#c2') and rect.x >= column.x and rect.x < column.x + column.w then seen[rect.colour] = true end
    end
    assert(seen['#c1'] and seen['#c2'], 'both series drawn in column ' .. index)
  end
end)
");
}

#[test]
fn a_live_revision_refreshes_a_pop_windows_title_and_place() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{subject = {kind = 'pop', id = '0000000000000001'}, graph = 'people'}
  assert(one.window.title == 'peasant (pagan) at Cong · people', one.window.title)
  -- The same pop id now sits at Armagh with nothing to tell it from others at its type.
  game.history_subjects = function()
    return {revision = 2, tags = {'global'}, places = {global = {'cong', 'armagh'}}, goods = {}, pops = {
      armagh = {{id = '0000000000000001', kind = 'peasant', culture = 'irish', religion = 'pagan', adds = 'none', ordinal = 0}}}}
  end
  local before = #requests
  graph.refresh{history_revision = 6, world = 0}
  assert(one.window.title == 'peasant at Armagh · people', one.window.title)
  assert(one.pick.kind == 'place' and one.pick.id == 'armagh', 'the picker follows the pop to its place')
  assert(#requests == before + 1 and requests[#requests].subject.id == '0000000000000001', 'still the same id')
end)
");
}

#[test]
fn a_closed_graph_window_lets_go_of_its_instance_and_answer() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local weak = setmetatable({}, {__mode = 'v'})
  for round = 1, 3 do
    local one = graph.open{subject = {kind = 'place', id = 'cong'}, graph = 'people'}
    assert(one:receive(answer(requests[#requests], 5, one:budget(), 36)), 'an answer is held')
    weak[round] = one
    weak[round + 10] = one.answer
    one.window:close()
    one = nil
  end
  assert(#graph.windows() == 0, 'the windows are forgotten')
  collectgarbage()
  collectgarbage()
  for key, value in pairs(weak) do error('still reachable after close: ' .. key) end
end)
");
}

#[test]
fn each_column_collapses_and_shows_on_its_own_with_its_own_strip_and_width() {
    // Review 1: any column's `<` hides that column only, leaves a 13 UI pixel strip with a `>` that shows only it, and the window narrows or
    // widens by that column's width less its strip.
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{columns = true}
  assert(#one.lists == 3 and one:chart_x() == 70 + 86 + 112 + 3 + 1, 'all shown: ' .. one:chart_x())
  local w = one.window:size()
  one:set_column(1, false)
  assert(#one.lists == 2 and one.shown[1] == false and one.shown[2] and one.shown[3], 'tags only is hidden')
  assert(w - one.window:size() == (70 - 13) * P, 'narrowed by the column less its strip')
  assert(one:chart_x() == 14 + 87 + 113 + 1, 'a strip in place of the column: ' .. one:chart_x())
  one:set_column(3, false)
  assert(#one.lists == 1 and one.shown[2], 'places stays')
  assert(w - one.window:size() == (70 - 13 + 112 - 13) * P)
  -- A strip's button shows only its own column again.
  one:set_column(1, true)
  assert(#one.lists == 2 and one.shown[1] and one.shown[2] and not one.shown[3])
  assert(w - one.window:size() == (112 - 13) * P, 'widened by the tags column less its strip')
  one:set_columns(false)
  assert(#one.lists == 0 and one:chart_x() == 3 * 14 + 1)
  assert(one.window:size() == w - graph.ALL_EXTRA * P)
  local keep = 0
  for _, entry in ipairs(one.buttons) do keep = keep + 1 end
  assert(keep == 3 + 4, 'three strips and four range buttons: ' .. keep)
end)
");
}

#[test]
fn equal_rates_stay_side_by_side_and_visible_as_the_window_crosses_the_legend_threshold() {
    // Review 2: births and deaths are two series in a column, so the budget and the legend rule leave a pixel for each: where the legend would
    // leave a plot too narrow for the longest history's columns at two pixels each, the legend goes and the plot widens.
    ok(r"
run(function()
  local spec = {height = 150, dim = '#dim', track = '#track', scale = 1,
    colour_of = function(index) return '#c' .. index end, name_of = function(key) return key end}
  assert(chart.least_plot(1) == 160 and chart.least_plot(2) == 186, 'room for both series of every century')
  for _, width in ipairs({201, 273, 300, 330, 400, 500}) do
    local per = 2
    local budget = chart.budget(width, per)
    local columns = {}
    local count = math.min(93, budget)
    for index = 1, count do
      columns[index] = {from = {year = 700 + index, month = 1}, to = {year = 700 + index, month = 12}, months = 12, record = true,
        values = {12.0, 12.0}, counts = {3, 3}, people = 400.0}
    end
    spec.width = width
    local layout = chart.layout({step = 'century', stacked = false, rate = true, series = {'births', 'deaths'}, columns = columns}, spec)
    if width >= 273 then assert(count == 93, 'the longest history fits at ' .. width .. ': ' .. budget) end
    if width >= 201 and width < 273 then assert(count < 93 or width == 201, 'a narrower window keeps within its pixels') end
    for index, column in ipairs(layout.columns) do
      local births, deaths
      for _, rect in ipairs(layout.rects) do
        if rect.x >= column.x and rect.x < column.x + column.w then
          if rect.colour == '#c1' then births = rect elseif rect.colour == '#c2' then deaths = rect end
        end
      end
      assert(births ~= nil and deaths ~= nil, 'both rates drawn at ' .. width .. ' column ' .. index)
      assert(births.x + births.w <= deaths.x or deaths.x + deaths.w <= births.x, 'side by side, never over each other: ' .. width .. ' column ' .. index)
    end
  end
  -- The legend goes only when the plot would otherwise be too narrow (rule 39).
  local _, wide = chart.plot_width(500, 2)
  local _, edge = chart.plot_width(273, 2)
  assert(wide and not edge, 'a wide window keeps its legend, the 273 pixel chart area of a 322 pixel window gives it up')
  -- The least window (three strips) still has room for both series of 93 centuries.
  assert(chart.budget(300 - 6 - 3 * 14 - 1, 2) >= 93, 'least window: ' .. chart.budget(300 - 6 - 3 * 14 - 1, 2))
end)
");
}

#[test]
fn a_monthly_refresh_keeps_a_picker_choice_the_player_made_and_follows_only_before_it() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{subject = {kind = 'pop', id = '0000000000000001'}, graph = 'people'}
  one:set_columns(true)
  assert(one.pick.id == 'cong', 'the picker starts at the displayed pop')
  one:choose_place({id = 'armagh', name = 'Armagh'})
  graph.refresh{history_revision = 6, world = 0}
  assert(one.pick.kind == 'place' and one.pick.id == 'armagh', 'a choice made by the player survives a monthly refresh')
  assert(one.subject.id == '0000000000000001', 'the displayed graph is still the same id')
  -- A tag choice survives too.
  one:choose_tag('global')
  graph.refresh{history_revision = 7, world = 0}
  assert(one.pick.kind == 'global' and one.tag == 'global')
end)
");
}

#[test]
fn a_pop_absent_from_the_latest_month_keeps_the_label_and_place_of_its_own_latest_record() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  -- Pop 9 is in no place of the latest month's subjects.
  local one = graph.open{subject = {kind = 'pop', id = '0000000000000009'}, graph = 'people'}
  assert(one.window.title == 'pop · people', 'unknown until its record arrives: ' .. one.window.title)
  local reply = answer(requests[1], 5, one:budget())
  reply.pop = {kind = 'peasant', culture = 'irish', religion = 'christian', place = 'armagh'}
  assert(one:receive(reply))
  assert(one.window.title == 'peasant at Armagh · people', one.window.title)
  graph.refresh{history_revision = 6, world = 0}
  assert(one.window.title == 'peasant at Armagh · people', 'a refresh does not lose it: ' .. one.window.title)
  assert(one.pick.id == 'armagh', 'the picker follows the pop to the place of its own record')
end)
");
}

#[test]
fn a_pops_title_from_its_own_latest_record_keeps_what_its_label_adds_and_its_number() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  -- Pop 9 is in no place of the latest month's subjects; its own latest record says what told it from the others at its place.
  local one = graph.open{subject = {kind = 'pop', id = '0000000000000009'}, graph = 'people'}
  local reply = answer(requests[1], 5, one:budget())
  reply.pop = {kind = 'peasant', culture = 'irish', religion = 'pagan', place = 'cong', adds = 'religion', ordinal = 0}
  assert(one:receive(reply))
  assert(one.window.title == 'peasant (pagan) at Cong · people', one.window.title)
  -- Two pops alike in everything but their number: the number stays in the title, as in the picker.
  local two = graph.open{subject = {kind = 'pop', id = '0000000000000008'}, graph = 'people'}
  local second = answer(requests[#requests], 5, two:budget())
  second.pop = {kind = 'peasant', culture = 'irish', religion = 'pagan', place = 'cong', adds = 'both', ordinal = 2}
  assert(two:receive(second))
  local expected = graph.pop_label{kind = 'peasant', culture = 'irish', religion = 'pagan', adds = 'both', ordinal = 2} .. ' at Cong · people'
  assert(two.window.title == expected, two.window.title)
  assert(expected:find('#2', 1, true), 'the ordinal is in the title: ' .. expected)
  graph.refresh{history_revision = 6, world = 0}
  assert(two.window.title == expected, 'a refresh does not lose it: ' .. two.window.title)
end)
");
}

#[test]
fn a_load_forgets_the_pop_the_old_history_named_and_resolves_the_id_in_the_new_one() {
    ok(r"
run(function()
  graph.refresh{history_revision = 5, world = 0}
  local one = graph.open{subject = {kind = 'pop', id = '0000000000000009'}, graph = 'people'}
  local reply = answer(requests[1], 5, one:budget())
  reply.pop = {kind = 'peasant', culture = 'irish', religion = 'pagan', place = 'armagh', adds = 'none', ordinal = 0}
  assert(one:receive(reply))
  assert(one.window.title == 'peasant at Armagh · people', one.window.title)
  graph.refresh{history_revision = 6, world = 0}
  assert(one.pick.kind == 'place' and one.pick.id == 'armagh', 'the picker follows the pop to the place of its own record')
  -- Another save is loaded: its history has no record of that id. The new revision answers `no records yet` with no pop.
  graph.refresh{history_revision = 1, world = 1}
  assert(one.known_pop == nil, 'the old world pop is not kept')
  assert(one.window.title == 'pop · people', 'nothing of the old world in the title: ' .. one.window.title)
  assert(one.pick.kind == 'global' and one.pick.id == 'global', 'the picker is no longer at the old world place')
  local empty = answer(requests[#requests], 1, one:budget())
  empty.columns = {}
  empty.message = 'no records yet'
  assert(one:receive(empty))
  assert(one.window.title == 'pop · people', one.window.title)
  -- A new world that does record the pop names it from its own record.
  graph.refresh{history_revision = 2, world = 2}
  local named = answer(requests[#requests], 2, one:budget())
  named.pop = {kind = 'monk', culture = 'irish', religion = 'christian', place = 'cong', adds = 'none', ordinal = 0}
  assert(one:receive(named))
  assert(one.window.title == 'monk at Cong · people', one.window.title)
end)
");
}
