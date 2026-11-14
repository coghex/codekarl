//! The list bar (CK-295, slice L2): `scripts/ui/lib/listbar.lua` in a real UI VM, in the script test rig, with no window.
//!
//! The shipped library runs over the real `render` API; a Lua shadow of the scene (the prelude's `scene`, `lines`) records every
//! quad, group offset, clip, hit region and text line the library makes, so a test reads what was drawn and where. Pointer cases
//! drive the real input path: Rust feeds the events, the script's frame hook dispatches them and calls `listbar.update` as
//! `main.lua` does. Nothing sleeps.

use ck_lua::ConfigValue;
use ck_window::{PointerEvent, ScrollUnit};
use std::fmt::Write;

use super::group_tests::Rig;
use super::widget_witness_tests::points;

/// The Lua every test starts with: the scene shadow, the text and tooltip observers, the frame hook of `main.lua` (dispatcher,
/// `listbar.update`, tooltips), the Escape callback, and small helpers. The window is 800 by 600 points.
pub(super) const PRELUDE: &str = r#"
local ui = require("lib.ui")
local pointer = require("lib.pointer")
local text = require("lib.text")
local theme = require("lib.theme")
P = ui.P
windows = require("lib.window")
lb = require("lib.listbar")
tooltip = require("lib.tooltip")
button = require("lib.button")
lists = require("lib.list")
pointer_lib = pointer
theme_lib = theme

function note(s) window.record(s) end
function expect(ok, message) if not ok then error(message or "expectation failed", 2) end end
function eq(actual, wanted, label)
  if actual ~= wanted then error((label or "value") .. ": got " .. tostring(actual) .. ", wanted " .. tostring(wanted), 2) end
end

-- The scene shadow.
scene = {quads = {}, groups = {}, hits = {}, seq = 0}
texture_names = {}
local real_texture = ui.texture
ui.texture = function(name)
  local token = real_texture(name)
  texture_names[token] = name
  return token
end
local real = {}
for _, name in ipairs({"create_group", "destroy_group", "set_group_offset", "set_group_clip", "create_quad", "destroy_quad",
    "set_rect", "set_position", "set_tint", "set_hit"}) do
  real[name] = render[name]
end
render.create_group = function()
  local group = real.create_group()
  scene.groups[group] = {x = 0, y = 0}
  return group
end
render.set_group_offset = function(group, x, y)
  real.set_group_offset(group, x, y)
  scene.groups[group].x, scene.groups[group].y = x, y
end
render.set_group_clip = function(group, clip)
  real.set_group_clip(group, clip)
  scene.groups[group].clip = clip
end
render.create_quad = function(spec)
  local quad = real.create_quad(spec)
  scene.seq = scene.seq + 1
  scene.quads[quad] = {id = quad, seq = scene.seq, texture = texture_names[spec.texture], x = spec.x, y = spec.y,
    w = spec.width, h = spec.height, tint = spec.tint, group = spec.group}
  return quad
end
render.set_rect = function(quad, x, y, w, h)
  real.set_rect(quad, x, y, w, h)
  local kept = scene.quads[quad]
  kept.x, kept.y, kept.w, kept.h = x, y, w, h
end
render.set_position = function(quad, x, y)
  real.set_position(quad, x, y)
  local kept = scene.quads[quad]
  kept.x, kept.y = x, y
end
render.set_tint = function(quad, tint)
  real.set_tint(quad, tint)
  scene.quads[quad].tint = tint
end
render.destroy_quad = function(quad)
  real.destroy_quad(quad)
  scene.quads[quad] = nil
end
render.set_hit = function(key, rects, group)
  real.set_hit(key, rects, group)
  scene.hits[key] = rects ~= nil and {rects = rects, group = group} or nil
end
render.destroy_group = function(group)
  real.destroy_group(group)
  scene.groups[group] = nil
  for id, kept in pairs(scene.quads) do if kept.group == group then scene.quads[id] = nil end end
  for key, kept in pairs(scene.hits) do if kept.group == group then scene.hits[key] = nil end end
end

-- The text lines.
lines = {}
local real_line, real_set, real_segments = text.line, text.set, text.set_segments
local real_colour, real_destroy = text.set_colour, text.destroy
text.line = function(x, y, group, colour, font_name)
  local line = real_line(x, y, group, colour, font_name)
  line.test_runs = {}
  lines[#lines + 1] = line
  return line
end
text.set = function(line, content)
  real_set(line, content)
  line.test_runs = {{text = content, colour = line.colour}}
end
text.set_segments = function(line, segments)
  real_segments(line, segments)
  line.test_runs = {}
  for _, segment in ipairs(segments) do line.test_runs[#line.test_runs + 1] = {text = segment.text, colour = segment.colour or line.colour} end
end
text.set_colour = function(line, colour)
  real_colour(line, colour)
  for _, run in ipairs(line.test_runs) do run.colour = colour end
end
text.destroy = function(line)
  real_destroy(line)
  line.test_dead = true
end

-- The one-line hover labels, by key.
labels = {}
local real_label = tooltip.label
tooltip.label = function(key, words)
  labels[key] = words
  return real_label(key, words)
end

function line_text(line)
  local parts = {}
  for _, run in ipairs(line.test_runs) do parts[#parts + 1] = run.text end
  return table.concat(parts)
end
-- Where a quad or a line is, in screen points: its own position and its group's offset.
function offset_of(group)
  local kept = group ~= nil and scene.groups[group] or nil
  return kept ~= nil and kept.x or 0, kept ~= nil and kept.y or 0
end
function line_at(line)
  local gx, gy = offset_of(line.group)
  return line.x + gx, line.y + gy
end
function quad_at(quad)
  local gx, gy = offset_of(quad.group)
  return quad.x + gx, quad.y + gy, quad.w, quad.h
end
-- The live text lines of a group that hold text, in creation order.
function lines_in(group)
  local found = {}
  for _, line in ipairs(lines) do
    if not line.test_dead and line.group == group and line_text(line) ~= "" then found[#found + 1] = line end
  end
  return found
end
-- The live quads of a group, in creation order, optionally of one texture.
function quads_in(group, texture)
  local found = {}
  for _, quad in pairs(scene.quads) do
    if quad.group == group and (texture == nil or quad.texture == texture) then found[#found + 1] = quad end
  end
  table.sort(found, function(a, b) return a.seq < b.seq end)
  return found
end
-- A hit region's first rectangle, in screen points.
function hit_rect(key)
  local kept = scene.hits[key]
  if kept == nil then return nil end
  local gx, gy = offset_of(kept.group)
  local rect = kept.rects[1]
  return rect.x + gx, rect.y + gy, rect.width, rect.height
end
function centre_of(x, y, w, h) return x + w / 2, y + h / 2 end
marked = {}
function mark(name, x, y)
  marked[name] = {x, y}
  note(string.format("%s %g %g", name, x, y))
end
function point_of(name) return marked[name][1], marked[name][2] end
function count_hits(prefix)
  local count = 0
  for key in pairs(scene.hits) do if key:sub(1, #prefix) == prefix then count = count + 1 end end
  return count
end

-- The frame hook of main.lua.
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  require("lib.settings_window").update(update)
  lb.update(update)
  tooltip.update(update)
  if on_frame ~= nil then on_frame(update) end
end)
window.on_close_request(function(source) return windows.close_request(source) end)

-- A window of the given content size in UI pixels, at (20, 20), of the `place` kind.
function new_window(width, height, kind)
  W = windows.new{title = "list", content_width = width * P, content_height = height * P, x = 20, y = 20, kind = kind or "place"}
  return W
end
function content_origin() return W:content_origin() end

-- A row of the fixtures.
function row(name, kind, people, extra)
  local r = {name = name, sort = {name = name, people = people}, filter = {kind = kind}}
  for key, value in pairs(extra or {}) do r[key] = value end
  return r
end
function names(rows)
  local out = {}
  for index, item in ipairs(rows) do out[index] = item.name end
  return table.concat(out, ",")
end
SORTS = {{key = "name", label = "name", first = "ascending"}, {key = "people", label = "people", first = "descending"}}
GROUPS = {{key = "kind", label = "kind"}}
function new_bar(extra)
  changes = 0
  local spec = {window = W, sorts = SORTS, groups = GROUPS, on_change = function() changes = changes + 1 end}
  for key, value in pairs(extra or {}) do spec[key] = value end
  bar = lb.new(spec)
  return bar
end
steps = {}
"#;

pub(super) fn script(body: &str, steps: &[&str]) -> String {
    let mut queued = String::new();
    for step in steps {
        writeln!(queued, "steps[#steps + 1] = function()\n{step}\nend")
            .expect("a string takes text");
    }
    format!(
        "{PRELUDE}\n{body}\n{queued}\nwindow.on_opened(function() table.remove(steps, 1)() end)\n"
    )
}

pub(super) fn rig(body: &str, steps: &[&str]) -> Rig {
    let mut rig = Rig::new_fixture_body(&script(body, steps));
    rig.deliver_fixture_frame_at(800, 600, 2);
    rig
}

pub(super) fn notes(rig: &Rig) -> Vec<String> {
    rig.tables
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(|value| match value {
            ConfigValue::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The last point the script marked as `name`.
pub(super) fn point(rig: &Rig, name: &str) -> (f64, f64) {
    let prefix = format!("{name} ");
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no mark {name}: {:?}", notes(rig)));
    let mut parts = line[prefix.len()..].split_whitespace();
    (
        parts.next().unwrap().parse().unwrap(),
        parts.next().unwrap().parse().unwrap(),
    )
}

pub(super) fn frame(rig: &mut Rig) {
    rig.deliver_fixture_frame_at(800, 600, 2);
}

pub(super) fn click_at(rig: &mut Rig, (x, y): (f64, f64)) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    frame(rig);
}

pub(super) fn click(rig: &mut Rig, name: &str) {
    let at = point(rig, name);
    click_at(rig, at);
}

pub(super) fn hover(rig: &mut Rig, (x, y): (f64, f64)) {
    rig.feed(&[PointerEvent::Move(points(x, y))]);
    frame(rig);
}

fn wheel(rig: &mut Rig, (x, y): (f64, f64), lines: i32) {
    rig.feed(&[PointerEvent::Scroll {
        position: points(x, y),
        amount: lines * 256,
        unit: ScrollUnit::Lines,
    }]);
    frame(rig);
}

/// The Lua that opens a picker as a press on its button does, and marks where things are for the pointer: the two buttons, the
/// open picker's rows, and a point over no key.
pub(super) const MARKS: &str = r#"
function marks()
  local ox, oy = content_origin()
  for name, b in pairs({filter = bar.filter_button, sort = bar.sort_button}) do
    local w, h = b:size()
    mark(name .. "_button", ox + b.x + w / 2, oy + b.y + h / 2)
  end
  if bar.picker ~= nil then
    local rows = bar.picker.rows
    for index, r in ipairs(rows) do
      if r.key ~= nil then
        local x, y, w, h = hit_rect(r.key)
        mark("row" .. index, x + w / 2, y + h / 2)
      end
    end
    local x, y, w, h = hit_rect(bar.picker.panel_key)
    mark("panel", x + w - 3, y + 2)
    mark("panel_corner", x + 1, y + h - 1)
  end
  mark("elsewhere", 700, 500)
end
"#;

// ---------------------------------------------------------------------------------------------
// The bar's choices (rules 5 to 7)
// ---------------------------------------------------------------------------------------------

#[test]
fn with_nothing_ticked_and_the_default_order_view_returns_the_callers_own_rows_in_their_order() {
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local rows = {row("Rath", "ringfort", 30), row("Nas", "royal_site", 10), row("Cruach", "crannog", 20)}
local shown = bar:view(rows)
expect(shown ~= rows, "a new array")
eq(#shown, 3)
for index = 1, 3 do expect(shown[index] == rows[index], "the caller's own table " .. index) end
eq(names(shown), "Rath,Nas,Cruach")
eq(changes, 0, "view is not a change")
"#],
    );
    rig.opened();
}

#[test]
fn the_filter_ors_within_a_group_ands_across_groups_and_leaves_a_group_with_nothing_ticked_alone() {
    let mut rig = rig(
        r#"
new_window(120, 100)
local religion = {}
new_bar{groups = {{key = "kind", label = "kind"}, {key = "faith", label = "faith"}}}
"#,
        &[r#"
local function r(name, kind, faith) return {name = name, filter = {kind = kind, faith = faith}} end
local rows = {
  r("a", "ringfort", "christian"), r("b", "royal_site", "pagan"), r("c", "crannog", "christian"),
  r("d", {"ringfort", "crannog"}, "christian"), r("e", nil, "christian"), r("f", "ringfort", nil),
}
bar:view(rows)
bar:toggle(1, "ringfort")
eq(names(bar:view(rows)), "a,d,f", "one box")
bar:toggle(1, "royal_site")
eq(names(bar:view(rows)), "a,b,d,f", "two boxes of one group show either value; an array passes on any of its values")
bar:toggle(1, "royal_site")
bar:toggle(2, "christian")
eq(names(bar:view(rows)), "a,d", "a box in each of two groups: both must match; a row with no value for a ticked group does not pass")
bar:clear()
eq(names(bar:view(rows)), "a,b,c,d,e,f", "nothing ticked excludes nothing")
bar:toggle(2, "pagan")
eq(names(bar:view(rows)), "b", "the second group alone")
"#],
    );
    rig.opened();
}

#[test]
fn a_number_sort_is_in_its_first_direction_then_the_other_with_stable_ties_and_missing_values_last()
{
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local rows = {
  row("a", "x", 5), row("b", "x", nil), row("c", "x", 30), row("d", "x", 5), row("e", "x", 100), row("f", "x", nil), row("g", "x", 9),
}
bar:view(rows)
bar:choose(2) -- people, descending first
eq(names(bar:view(rows)), "e,c,g,a,d,b,f", "most first; equal values in the given order; no value last, in the given order")
bar:choose(2)
eq(names(bar:view(rows)), "a,d,g,c,e,b,f", "pressed again: fewest first; ties still in the given order; no value still last")
bar:choose(nil)
eq(names(bar:view(rows)), "a,b,c,d,e,f,g", "default restores the given order")
bar:choose(nil)
eq(changes, 3, "default when chosen does nothing")
-- Numbers compare as numbers, not as text: 9 before 100.
bar:choose(2) bar:choose(2)
local ordered = bar:view(rows)
eq(ordered[3].name .. ordered[4].name .. ordered[5].name, "gce", "9, 30, 100: as numbers, not as text")
"#],
    );
    rig.opened();
}

#[test]
fn a_text_sort_ignores_case_and_counts_the_acute_vowels_as_plain_ones() {
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local rows = {row("\u{C1}th Cliath", "x", 1), row("an N\u{E1}s", "x", 1), row("Eamhain", "x", 1), row("Ard Macha", "x", 1)}
bar:view(rows)
bar:choose(1)
eq(names(bar:view(rows)), "an N\u{E1}s,Ard Macha,\u{C1}th Cliath,Eamhain")
bar:choose(1)
eq(names(bar:view(rows)), "Eamhain,\u{C1}th Cliath,Ard Macha,an N\u{E1}s", "descending is the same order reversed")
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// The buttons and the count (rules 8 and 9)
// ---------------------------------------------------------------------------------------------

#[test]
fn the_two_buttons_sit_under_the_list_at_the_content_edge_with_their_labels_and_are_lit_by_the_choices()
 {
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local ox, oy = content_origin()
local filter, sort = bar.filter_button, bar.sort_button
local fw, fh = filter:size()
local sw, sh = sort:size()
eq(fw, 13 * P) eq(fh, 15 * P) eq(sw, 13 * P) eq(sh, 15 * P)
-- The content is 120 by 100 UI pixels: the sort button's right edge is the content's, the filter button is 16 pixels left of it, and
-- both tops are 17 pixels above the content's bottom edge.
eq(sort.x + sw, 120 * P, "sort right edge")
eq(filter.x, sort.x - 16 * P, "filter 16 pixels left")
eq(sort.y, (100 - 17) * P, "top 17 above the bottom")
eq(filter.y, sort.y)
local x, y, w, h = hit_rect(sort.key)
eq(x, ox + sort.x) eq(y, oy + sort.y) eq(w, 13 * P) eq(h, 15 * P)
eq(labels[filter.key], "filter") eq(labels[sort.key], "sort")
expect(filter.selected ~= true and sort.selected ~= true, "neither lit at the start")
local rows = {row("a", "ringfort", 1), row("b", "crannog", 2)}
bar:view(rows)
bar:toggle(1, "ringfort")
expect(filter.selected == true and sort.selected ~= true, "a ticked box lights the filter button only")
bar:toggle(1, "ringfort")
expect(filter.selected ~= true, "unticked: unlit")
bar:choose(1)
expect(sort.selected == true and filter.selected ~= true, "a chosen sort lights the sort button")
bar:choose(1)
expect(sort.selected == true, "the other direction is still not the default order")
bar:choose(nil)
expect(sort.selected ~= true, "the default order: unlit")
-- A window of another size moves them with it.
W:set_content_size(80 * P, 60 * P)
eq(bar.sort_button.x + sw, 80 * P) eq(bar.sort_button.y, 43 * P)
"#],
    );
    rig.opened();
}

#[test]
fn the_count_reads_n_of_m_beside_the_filter_button_only_while_a_box_is_ticked() {
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local rows = {row("a", "ringfort", 1), row("b", "crannog", 2), row("c", "ringfort", 3)}
bar:view(rows)
eq(line_text(bar.count), "", "nothing drawn with nothing ticked")
eq(#bar.count.glyphs, 0)
bar:toggle(1, "ringfort")
bar:view(rows)
eq(line_text(bar.count), "2 of 3")
local runs = bar.count.test_runs
eq(runs[1].text, "2") eq(runs[1].colour, theme_lib.colour("number"))
eq(runs[2].text, " of ") eq(runs[2].colour, theme_lib.colour("dim"))
eq(runs[3].text, "3") eq(runs[3].colour, theme_lib.colour("number"))
local x, y = line_at(bar.count)
local ox, oy = content_origin()
eq(x + bar.count.width * P, ox + bar.filter_button.x - 5 * P, "its right end is 5 pixels left of the filter button")
eq(y, oy + bar.filter_button.y + 2 * P, "its top is 2 pixels below the buttons' top")
bar:view({row("a", "ringfort", 1)})
eq(line_text(bar.count), "1 of 1", "the numbers of the last view")
bar:clear()
bar:view(rows)
eq(#bar.count.glyphs, 0, "unticked: nothing drawn")
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// The filter picker (rules 10, 11, 13)
// ---------------------------------------------------------------------------------------------

#[test]
fn the_filter_picker_of_one_group_is_a_titleless_panel_with_an_all_button_and_the_boxes_in_label_order()
 {
    let mut rig = rig(
        &format!(r"new_window(120, 100) new_bar() {MARKS}"),
        &[r#"
bar:view({row("a", "royal_site", 1), row("b", "ringfort", 2), row("c", "crannog", 3), row("d", "monastery", 4), row("e", "ringfort", 5)})
bar.filter_button.on_click()
local picker = bar.picker
expect(picker ~= nil and picker.kind == "filter")
local ox, oy = content_origin()
local function px(font_text) return font.layout(font_text).width end
local widest = math.max(px("royal site") + 12, (button.measure("all")) // P)
eq(picker.content_width, (widest + 4) * P, "the content is 4 wider than the widest")
local width, height = picker.content_width + 8 * P, (16 + 4 * 13) * P + 6 * P
local x, y, w, h = hit_rect(picker.panel_key)
eq(w, width, "8 wider than its content") eq(h, height, "6 taller than its content")
eq(x + w, ox + 120 * P, "its right edge is the content's right edge")
eq(y + h, oy + (100 - 17) * P - P, "one clear pixel above the buttons' top")
-- The all button, at the content's top left (4 from the left, 3 from the top).
local gx, gy = offset_of(picker.group)
eq(gx, x) eq(gy, y)
eq(picker.all.x, 4 * P) eq(picker.all.y, 3 * P)
expect(picker.all.selected == true, "all is lit with nothing ticked")
-- Rows from 16 pixels below the content's top, 13 high.
local rx, ry = offset_of(picker.rows_group)
eq(rx, x + 4 * P) eq(ry, y + 3 * P + 16 * P)
local boxes = {"crannog", "monastery", "ringfort", "royal site"}
local found = lines_in(picker.rows_group)
eq(#found, 4, "no heading with one group")
for index, label in ipairs(boxes) do
  eq(line_text(found[index]), label, "box " .. index)
  local lx, ly = line_at(found[index])
  eq(lx, rx + 12 * P) eq(ly, ry + (index - 1) * 13 * P + P)
  eq(found[index].test_runs[1].colour, picker.colours.text)
end
-- The first box's outline: 7 by 7, one pixel thick, 1 in and 3 down.
local outline = quads_in(picker.rows_group, "ui/theme/white.png")
local seen = {}
for _, quad in ipairs(outline) do
  local qx, qy, qw, qh = quad_at(quad)
  if qy >= ry and qy < ry + 13 * P and qx >= rx and qx < rx + 8 * P and qw < 8 * P then
    seen[#seen + 1] = string.format("%g,%g,%g,%g", qx - rx, qy - ry, qw, qh)
  end
end
table.sort(seen)
local wanted = {"2,6,14,2", "2,18,14,2", "14,8,2,10", "2,8,2,10"}
table.sort(wanted)
eq(table.concat(seen, ";"), table.concat(wanted, ";"), "a 7 by 7 outline one pixel thick, 1 in and 3 down")
"#],
    );
    rig.opened();
}

#[test]
fn the_filter_picker_of_three_groups_has_a_heading_row_before_each_and_every_row_in_place() {
    let mut rig = rig(
        r#"
new_window(120, 200)
new_bar{groups = {{key = "kind", label = "type"}, {key = "faith", label = "religion"}, {key = "tribe", label = "culture"}}}
"#,
        &[r#"
local function r(name, kind, faith, tribe) return {name = name, filter = {kind = kind, faith = faith, tribe = tribe}} end
bar:view({r("a", "monk", "christian", "laigin"), r("b", "peasant", "pagan", "ulaid"), r("c", "peasant", "christian", "laigin")})
bar.filter_button.on_click()
local picker = bar.picker
local rx, ry = offset_of(picker.rows_group)
local found = lines_in(picker.rows_group)
local expected = {"type", "monk", "peasant", "religion", "christian", "pagan", "culture", "laigin", "ulaid"}
eq(#found, #expected)
for index, label in ipairs(expected) do
  eq(line_text(found[index]), label, "row " .. index)
  local x, y = line_at(found[index])
  eq(y, ry + (index - 1) * 13 * P + P, "one pixel below the row's top: " .. label)
  local heading = label == "type" or label == "religion" or label == "culture"
  eq(x, rx + (heading and P or 12 * P), label)
  eq(found[index].test_runs[1].colour, heading and theme_lib.colour("dim") or picker.colours.text, label .. " colour")
end
-- 16 pixels for the all button, then nine rows.
local _, _, _, h = hit_rect(picker.panel_key)
eq(h, (16 + 9 * 13) * P + 6 * P)
"#],
    );
    rig.opened();
}

#[test]
fn a_ticked_box_draws_its_centre_square_and_all_is_lit_only_with_nothing_ticked() {
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local rows = {row("a", "ringfort", 1), row("b", "crannog", 2)}
bar:view(rows)
bar.filter_button.on_click()
local picker = bar.picker
local function squares()
  local found = {}
  for _, quad in ipairs(quads_in(picker.rows_group, "ui/theme/white.png")) do
    if quad.w == 3 * P and quad.h == 3 * P then found[#found + 1] = quad end
  end
  return found
end
eq(#squares(), 0, "no centre square with nothing ticked")
expect(picker.all.selected == true)
bar:toggle(1, "ringfort") -- the second box (crannog, ringfort)
local found = squares()
eq(#found, 1, "one centre square")
local rx, ry = offset_of(picker.rows_group)
local qx, qy = quad_at(found[1])
eq(qx, rx + 3 * P) eq(qy, ry + 13 * P + 5 * P, "the 3 by 3 centre of the second box's 7 by 7 outline")
eq(found[1].tint, picker.colours.text, "in the text colour")
expect(picker.all.selected ~= true, "all is not lit with a box ticked")
bar:toggle(1, "ringfort")
eq(#squares(), 0, "unticked: the square goes")
expect(picker.all.selected == true)
"#],
    );
    rig.opened();
}

#[test]
fn all_unticks_every_box_and_calls_on_change_once_and_only_when_a_box_was_ticked() {
    let mut rig = rig(
        &format!(r"new_window(120, 100) new_bar() {MARKS}"),
        &[
            r#"
local rows = {row("a", "ringfort", 1), row("b", "crannog", 2)}
bar:view(rows)
bar.filter_button.on_click()
bar:toggle(1, "ringfort")
bar:toggle(1, "crannog")
eq(changes, 2)
local x, y, w, h = hit_rect(bar.picker.all.key)
mark("all", x + w / 2, y + h / 2)
"#,
            r#"
eq(bar.ticks, 0, "every box unticked")
eq(changes, 3, "on_change once for the press")
expect(bar.picker ~= nil, "the picker stays open")
expect(bar.picker.all.selected == true)
expect(bar.filter_button.selected ~= true)
"#,
            r#"
-- Pressing it with nothing ticked changes nothing.
local x, y, w, h = hit_rect(bar.picker.all.key)
mark("all", x + w / 2, y + h / 2)
"#,
            r#"eq(changes, 3, "nothing ticked: no change")"#,
        ],
    );
    rig.opened();
    click(&mut rig, "all");
    rig.opened();
    rig.opened();
    click(&mut rig, "all");
    rig.opened();
}

#[test]
fn a_value_that_leaves_the_rows_stays_a_box_while_ticked_and_a_new_value_appears_in_an_open_picker()
{
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
local function boxes()
  local out = {}
  for _, entry in ipairs(bar.picker.entries) do out[#out + 1] = entry.label end
  return table.concat(out, ",")
end
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
bar.filter_button.on_click()
eq(boxes(), "crannog,ringfort")
bar:toggle(1, "crannog")
-- The crannog leaves the rows: its box stays while it is ticked.
bar:view({row("a", "ringfort", 1)})
eq(boxes(), "crannog,ringfort", "ticked, so it stays")
bar:toggle(1, "crannog")
bar:view({row("a", "ringfort", 1)})
eq(boxes(), "ringfort", "unticked, so it goes")
-- A value new to the rows appears in the open picker after the next view.
local before = bar.picker
bar:view({row("a", "ringfort", 1), row("c", "royal_site", 3)})
eq(boxes(), "ringfort,royal site")
expect(bar.picker ~= before and bar.picker.kind == "filter" and not bar.picker.closed, "the picker is made again, and is open")
expect(before.closed, "the old one is gone")
eq(count_hits("listbar_box"), 2, "the old picker keys are gone")
-- With the boxes the same the picker is kept.
local kept = bar.picker
bar:view({row("a", "ringfort", 1), row("c", "royal_site", 3)})
expect(bar.picker == kept, "the same boxes keep the same picker")
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// The sort picker (rules 10, 12)
// ---------------------------------------------------------------------------------------------

#[test]
fn the_sort_picker_fills_the_chosen_row_marks_its_direction_and_turns_it_round_or_restores_the_default()
 {
    let mut rig = rig(
        r"new_window(120, 100) new_bar()",
        &[r#"
bar:view({row("a", "x", 1)})
bar.sort_button.on_click()
local picker = bar.picker
expect(picker ~= nil and picker.kind == "sort" and bar.filter_button.selected ~= true)
local ox, oy = content_origin()
local widest = math.max(font.layout("default").width, font.layout("name").width, font.layout("people").width)
eq(picker.content_width, (widest + 22) * P, "the content is 22 wider than the widest label")
local x, y, w, h = hit_rect(picker.panel_key)
eq(w, picker.content_width + 8 * P) eq(h, 3 * 13 * P + 6 * P, "three rows")
eq(x + w, ox + 120 * P) eq(y + h, oy + (100 - 17) * P - P)
local rx, ry = offset_of(picker.rows_group)
eq(rx, x + 4 * P) eq(ry, y + 3 * P)
local found = lines_in(picker.rows_group)
eq(#found, 3)
local labels_shown = {"default", "name", "people"}
for index, label in ipairs(labels_shown) do
  eq(line_text(found[index]), label)
  local lx, ly = line_at(found[index])
  eq(lx, rx + P) eq(ly, ry + (index - 1) * 13 * P + P)
end
local function fill()
  local qx, qy, qw, qh = quad_at(scene.quads[picker.chosen_quad])
  return qx, qy, qw, qh
end
local function ink(index) return found[index].test_runs[1].colour end
-- The default order: the "default" row is the chosen one, with no mark.
local fx, fy, fw, fh = fill()
eq(fx, x + P, "between the panel's side lines") eq(fw, w - 2 * P) eq(fy, ry) eq(fh, 13 * P)
eq(ink(1), picker.colours.pressed_text) eq(ink(2), picker.colours.text) eq(ink(3), picker.colours.text)
eq(#quads_in(picker.rows_group, "ui/theme/sort_up.png") + #quads_in(picker.rows_group, "ui/theme/sort_down.png"), 0, "default has no mark")
-- people, first descending: the third row, sort_down, 4 down and 6 left of the content's right edge.
bar:choose(2)
fx, fy = fill()
eq(fy, ry + 2 * 13 * P, "the chosen row is filled") eq(ink(3), picker.colours.pressed_text) eq(ink(1), picker.colours.text)
local down = quads_in(picker.rows_group, "ui/theme/sort_down.png")
eq(#down, 1) eq(#quads_in(picker.rows_group, "ui/theme/sort_up.png"), 0)
local mx, my, mw, mh = quad_at(down[1])
eq(mx, rx + picker.content_width - 6 * P) eq(my, ry + 2 * 13 * P + 4 * P) eq(mw, 5 * P) eq(mh, 5 * P)
eq(down[1].tint, picker.colours.pressed_text, "the mark is in the text's colour")
bar:choose(2)
eq(#quads_in(picker.rows_group, "ui/theme/sort_up.png"), 1, "turned round: ascending is sort_up")
eq(#quads_in(picker.rows_group, "ui/theme/sort_down.png"), 0)
bar:choose(1)
fx, fy = fill()
eq(fy, ry + 13 * P, "name, in its first direction: ascending")
eq(#quads_in(picker.rows_group, "ui/theme/sort_up.png"), 1)
bar:choose(nil)
fx, fy = fill()
eq(fy, ry, "default restores the first row")
eq(#quads_in(picker.rows_group, "ui/theme/sort_up.png") + #quads_in(picker.rows_group, "ui/theme/sort_down.png"), 0)
"#],
    );
    rig.opened();
}

#[test]
fn pressing_a_sort_row_chooses_it_and_the_chosen_row_again_turns_it_round_and_default_when_chosen_does_nothing()
 {
    let mut rig = rig(
        &format!(r"new_window(120, 100) new_bar() {MARKS}"),
        &[
            r#"
bar:view({row("b", "x", 1), row("a", "x", 2)})
bar.sort_button.on_click()
marks()
"#,
            r#"
eq(bar.sort_index, 2, "the people row: chosen in its first direction") eq(bar.direction, "descending") eq(changes, 1)
expect(bar.picker ~= nil, "the picker stays open after a functional press")
marks()
"#,
            r#"
eq(bar.sort_index, 2) eq(bar.direction, "ascending", "the chosen row again turns it round") eq(changes, 2)
marks()
"#,
            r#"
eq(bar.sort_index, nil, "default restores") eq(changes, 3)
marks()
"#,
            r#"eq(changes, 3, "default when chosen does nothing")"#,
        ],
    );
    rig.opened();
    click(&mut rig, "row3");
    rig.opened();
    click(&mut rig, "row3");
    rig.opened();
    click(&mut rig, "row1");
    rig.opened();
    click(&mut rig, "row1");
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// Hover, change and tooltips (rule 13)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_real_hover_over_a_box_row_and_a_sort_row_fills_it_with_row_hover_and_leaving_removes_it() {
    let mut rig = rig(
        &format!(r"new_window(120, 100) new_bar() {MARKS}"),
        &[
            r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
bar.filter_button.on_click()
marks()
"#,
            r#"
local picker = bar.picker
local qx, qy, qw, qh = quad_at(scene.quads[picker.hover_quad])
local rx, ry = offset_of(picker.rows_group)
eq(qy, ry + 13 * P, "the second box row (ringfort) is filled")
eq(qx, offset_of(picker.group) + P, "between the panel's side lines")
eq(qh, 13 * P)
eq(scene.quads[picker.hover_quad].tint, picker.colours.row_hover)
bar.sort_button.on_click()
marks()
"#,
            r#"
local picker = bar.picker
local qx, qy = quad_at(scene.quads[picker.hover_quad])
local rx, ry = offset_of(picker.rows_group)
eq(qy, ry + 2 * 13 * P, "the people row")
"#,
            r#"
local qx, qy = quad_at(scene.quads[bar.picker.hover_quad])
expect(qy < -1e6, "leaving removes the fill")
bar:choose(1)
marks()
"#,
            r#"
local qx, qy = quad_at(scene.quads[bar.picker.hover_quad])
expect(qy < -1e6, "the chosen row keeps its own fill, with no hover fill over it")
local cx, cy = quad_at(scene.quads[bar.picker.chosen_quad])
expect(cy > -1e6, "and the chosen fill is there")
"#,
        ],
    );
    rig.opened();
    let ringfort = point(&rig, "row2");
    hover(&mut rig, ringfort);
    rig.opened();
    let people = point(&rig, "row3");
    hover(&mut rig, people);
    rig.opened();
    hover(&mut rig, (700.0, 500.0));
    rig.opened();
    let name = point(&rig, "row2");
    hover(&mut rig, name);
    rig.opened();
}

pub(super) const PROBE: &str = r#"
-- A key outside the window with a lockable tooltip, for the tooltip cases.
render.set_hit("probe", {{x = 600, y = 400, width = 60, height = 40}})
tooltip.attach("probe", function() return {"probe"} end)
pointer_lib.register("probe", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
"#;

#[test]
fn a_change_closes_an_open_locked_tooltip_and_opening_a_picker_does_not() {
    let mut rig = rig(
        &format!(r"new_window(120, 100) new_bar() {MARKS} {PROBE}"),
        &[
            r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
marks()
mark("probe", 630, 420)
"#,
            r#"
eq(count_hits("tooltip_panel_"), 1, "the tooltip is open over the probe")
"#,
            r#"
eq(count_hits("tooltip_panel_"), 1, "it was locked, so moving off does not close it")
"#,
            r#"
eq(count_hits("tooltip_panel_"), 1, "opening a picker closes no tooltip")
expect(bar.picker ~= nil)
marks()
"#,
            r#"
eq(changes, 1)
eq(count_hits("tooltip_panel_"), 0, "a change closes every open tooltip")
expect(bar.picker ~= nil, "and the picker stays open")
"#,
        ],
    );
    rig.opened();
    let probe = point(&rig, "probe");
    hover(&mut rig, probe);
    for _ in 0..150 {
        frame(&mut rig);
    }
    rig.opened();
    hover(&mut rig, (700.0, 550.0));
    for _ in 0..5 {
        frame(&mut rig);
    }
    rig.opened();
    click(&mut rig, "filter_button");
    rig.opened();
    click(&mut rig, "row1");
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// Too tall (rule 13)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_picker_whose_rows_do_not_fit_is_as_tall_as_the_content_leaves_and_its_rows_scroll_under_the_all_button()
 {
    let mut rig = rig(
        &format!(
            r#"
new_window(120, 100)
list = lists.new{{window = W, x = 0, y = 0, width = 120 * P, rows = 2, items = {{"1", "2", "3", "4", "5", "6", "7", "8", "9", "10"}}}}
new_bar()
{MARKS}
"#
        ),
        &[
            r#"
local rows = {}
for index = 1, 12 do rows[index] = row("r" .. index, string.format("k%02d", index), index) end
bar:view(rows)
bar.filter_button.on_click()
local picker = bar.picker
local ox, oy = content_origin()
local x, y, w, h = hit_rect(picker.panel_key)
eq(y, oy, "the panel's top is the content's top")
eq(h, (100 - 18) * P, "as tall as that leaves")
eq(y + h, oy + (100 - 17) * P - P, "its bottom is still a pixel above the buttons")
local all_x, all_y = quad_at({x = picker.all.x, y = picker.all.y, group = picker.group})
mark("all_at", all_x, all_y)
local rx, ry = offset_of(picker.rows_group)
mark("rows_at", rx, ry)
local clip = scene.groups[picker.rows_group].clip
eq(clip.y, y + 3 * P + 16 * P, "the rows are clipped under the all button")
eq(clip.height, h - 6 * P - 16 * P)
eq(clip.x, x + P) eq(clip.width, w - 2 * P)
eq(list:scroll_position(), 0)
marks()
"#,
            r#"
local picker = bar.picker
local ox, oy = content_origin()
local x, y, w, h = hit_rect(picker.panel_key)
local all_x, all_y = quad_at({x = picker.all.x, y = picker.all.y, group = picker.group})
local ax, ay = point_of("all_at")
eq(all_x, ax) eq(all_y, ay, "the all button does not move")
local rx, ry = offset_of(picker.rows_group)
local bx, by = point_of("rows_at")
eq(ry, by - 13 * P, "one wheel line moves the rows 13 pixels")
eq(list:scroll_position(), 0, "the list under the picker stays where it was")
-- The scroll is limited at the last row.
for _ = 1, 40 do picker:scroll({kind = "scroll", unit = "lines", amount = -1}) end
local _, last = offset_of(picker.rows_group)
eq(last, by - (12 * 13 - (82 - 6 - 16)) * P, "scrolled to the end and no further")
for _ = 1, 40 do picker:scroll({kind = "scroll", unit = "lines", amount = 1}) end
local _, first = offset_of(picker.rows_group)
eq(first, by, "and back to the start and no further")
"#,
        ],
    );
    rig.opened();
    let rows_group_row = point(&rig, "row1");
    // The wheel over a row of the picker, which is over the list.
    wheel(&mut rig, rows_group_row, -1);
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// A list may choose the ticks its filter starts with (CK-304)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_bar_made_with_no_starting_ticks_or_an_empty_list_of_them_starts_with_nothing_ticked_as_before()
{
    let mut rig = rig(
        r"new_window(120, 100)",
        &[r#"
for _, extra in ipairs({{}, {ticked = {}}}) do
  new_bar(extra)
  eq(bar.ticks, 0, "nothing ticked")
  expect(bar.filter_button.selected ~= true, "the filter button is not lit")
  local rows = {row("a", "ringfort", 1), row("b", "crannog", 2)}
  eq(names(bar:view(rows)), "a,b", "everything shows")
  eq(line_text(bar.count), "", "no count")
  bar:destroy()
end
expect(not pcall(new_bar, {ticked = "none"}), "the argument is checked: it is a list or nothing")
"#],
    );
    rig.opened();
}

#[test]
fn a_bar_made_with_one_starting_tick_is_one_the_player_ticked_by_hand_and_all_clears_it() {
    let mut rig = rig(
        r#"new_window(120, 100) new_bar{ticked = {{group = "kind", value = "ringfort"}}}"#,
        &[r#"
eq(bar.ticks, 1, "one box ticked")
expect(bar.filter_button.selected == true, "the filter button is drawn selected at once")
eq(changes, 0, "making the bar is not a change: no callback")
local rows = {row("a", "ringfort", 1), row("b", "crannog", 2), row("c", "ringfort", 3)}
eq(names(bar:view(rows)), "a,c", "the rows are filtered by the ordinary rule")
eq(line_text(bar.count), "2 of 3", "the count shows")
-- The picker shows the box ticked and "all" not selected.
bar.filter_button.on_click()
local picker = bar.picker
local ticked = {}
for _, r in ipairs(picker.rows) do
  if r.entry.kind == "box" then ticked[r.entry.value] = r.tick ~= nil end
end
eq(ticked.ringfort, true) eq(ticked.crannog, false)
expect(picker.all.selected ~= true, "all is not the chosen look")
-- "all" clears it, as for a tick made by hand.
bar:clear()
eq(bar.ticks, 0)
expect(bar.filter_button.selected ~= true, "unlit")
eq(names(bar:view(rows)), "a,b,c")
eq(line_text(bar.count), "")
eq(changes, 1, "the clear is a change")
"#],
    );
    rig.opened();
}

#[test]
fn a_new_bar_for_the_same_list_starts_from_its_ticks_again_whatever_became_of_the_last() {
    let mut rig = rig(
        r"new_window(120, 100)",
        &[r#"
local start = {{group = "kind", value = "ringfort"}, {group = "kind", value = "crannog"}, {group = "kind", value = "ringfort"}}
new_bar{ticked = start}
eq(bar.ticks, 2, "a box given twice is one box")
bar:clear()
eq(bar.ticks, 0)
bar:destroy()
eq(#start, 3, "the caller's list is not changed") eq(start[1].value, "ringfort")
new_bar{ticked = start}
eq(bar.ticks, 2, "the next bar starts from them again")
expect(bar.filter_button.selected == true, "lit again")
bar:toggle(1, "crannog")
eq(bar.ticks, 1)
bar:destroy()
new_bar{ticked = start}
eq(bar.ticks, 2, "and again, after a tick by hand")
"#],
    );
    rig.opened();
}

#[test]
fn a_starting_tick_no_row_has_is_still_a_ticked_box_and_a_bad_ticked_list_is_refused_creating_nothing()
 {
    let mut rig = rig(
        r"new_window(120, 100)",
        &[r#"
new_bar{ticked = {{group = "kind", value = "monastery"}}}
local rows = {row("a", "ringfort", 1)}
eq(names(bar:view(rows)), "", "nothing matches the ticked value")
bar.filter_button.on_click()
local found = false
for _, r in ipairs(bar.picker.rows) do
  if r.entry.kind == "box" and r.entry.value == "monastery" then found = r.tick ~= nil end
end
expect(found, "the value is a box, ticked, as for one ticked by hand")
bar:destroy()
local before = #lines
for _, bad in ipairs({
  {ticked = 5}, {ticked = {{group = "nope", value = "x"}}}, {ticked = {{group = "kind"}}},
  {ticked = {{group = "kind", value = 3}}}, {ticked = {"kind"}},
}) do
  local ok = pcall(new_bar, bad)
  expect(not ok, "refused")
end
eq(#lines, before, "no line was made by a refused bar")
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// The scroll bar of a picker whose rows overflow (CK-299)
// ---------------------------------------------------------------------------------------------

/// A window of 120 by 100 UI pixels with a list under the bar, and a bar over `count` kinds; `picker_marks()` marks the thumb, the
/// track below it and a box row of the open picker.
fn bar_body(count: u32) -> String {
    format!(
        r#"
new_window(120, 100)
list = lists.new{{window = W, x = 0, y = 0, width = 120 * P, rows = 2, items = {{"1", "2", "3", "4", "5", "6", "7", "8", "9", "10"}}}}
new_bar()
{MARKS}
function kinds(n)
  local rows = {{}}
  for index = 1, n do rows[index] = row("r" .. index, string.format("k%02d", index), index) end
  return rows
end
function panel_rect() return hit_rect(bar.picker.panel_key) end
-- How many of a scroll bar's two hit regions exist (the list under the picker has a bar of its own).
function bar_hits(sb)
  local n = 0
  for _, key in ipairs({{sb.track_key or "none", sb.thumb_key or "none"}}) do
    if scene.hits[key] ~= nil then n = n + 1 end
  end
  return n
end
function picker_marks()
  local picker = bar.picker
  local sb = picker.scroll_bar
  local x, y, w, h = hit_rect(sb.thumb_key)
  mark("thumb", x + 3, y + 4)
  local px, py, pw, ph = panel_rect()
  mark("track", x + 3, py + ph - 8)
  mark("viewport_row", x - 20, y + 20)
  marks()
end
bar:view(kinds({count}))
"#
    )
}

#[test]
fn a_filter_picker_too_tall_for_its_room_has_the_bar_over_its_rows_its_panel_5_pixels_wider_and_all_in_place()
 {
    let mut rig = rig(
        &bar_body(12),
        &[r#"
bar.filter_button.on_click()
local picker = bar.picker
local ox, oy = content_origin()
local x, y, w, h = panel_rect()
eq(w, picker.content_width + 8 * P + 5 * P, "the panel is 5 UI pixels wider than a picker that fits")
eq(x + w, ox + 120 * P, "and grew to the left: its right edge is still the content's")
eq(h, (100 - 18) * P, "as tall as the room")
local sb = picker.scroll_bar
eq(#quads_in(sb.group), 2, "a track and a thumb")
local track = quads_in(sb.group)[1]
local tx, ty, tw, th = quad_at(track)
eq(tw, 3 * P) eq(tx, x + w - 4 * P, "at the right end of the rows' viewport")
eq(ty, y + 3 * P + 16 * P, "under the all button")
eq(th, h - 6 * P - 16 * P, "as tall as the viewport")
local all_x, all_y = quad_at({x = picker.all.x, y = picker.all.y, group = picker.group})
eq(all_x, x + 4 * P) eq(all_y, y + 3 * P, "the all button is where it was in the panel")
-- The rows keep their width: a row's region ends where it did, the bar stands in the room the panel grew by.
local rx, ry, rw, rh = hit_rect(picker.rows[2].key)
eq(rx + rw, x + w - 6 * P, "the box rows end 5 UI pixels before the panel's right edge, a gap of 2 before the track")
-- A picker that fits is as wide as it was and has no bar, and the one it replaced took its regions with it.
eq(bar_hits(sb), 2)
bar:view(kinds(2))
local x2, y2, w2, h2 = panel_rect()
eq(w2, bar.picker.content_width + 8 * P)
eq(bar_hits(sb), 0, "the old picker's regions are gone")
eq(bar_hits(bar.picker.scroll_bar), 0)
eq(#quads_in(bar.picker.scroll_bar.group), 0)
"#],
    );
    rig.opened();
}

#[test]
fn the_thumb_the_track_and_the_wheel_scroll_the_pickers_rows_and_not_the_list_and_a_press_on_the_bar_leaves_it_open()
 {
    let mut rig = rig(
        &bar_body(12),
        &[
            r#"
bar.filter_button.on_click()
picker_marks()
local rx, ry = offset_of(bar.picker.rows_group)
mark("rows_y", rx, ry)
"#,
            r#"
local picker = bar.picker
local rx, ry = offset_of(picker.rows_group)
local _, y0 = point_of("rows_y")
eq(ry, y0 - 13 * P, "one wheel line over the thumb moves the rows 13 pixels")
eq(list:scroll_position(), 0, "and the list under the picker stays where it was")
expect(bar.picker ~= nil, "the picker is open")
local x, y = hit_rect(picker.scroll_bar.thumb_key)
mark("thumb", x + 3, y + 4)
"#,
            r#"
local picker = bar.picker
expect(picker ~= nil, "a press on the thumb leaves the picker open")
local _, y0 = point_of("rows_y")
local rx, ry = offset_of(picker.rows_group)
expect(ry < y0 - 13 * P, "the drag scrolled the rows further: " .. ry .. " from " .. y0)
eq(list:scroll_position(), 0)
-- The thumb is where the picker's scroll puts it.
local viewport, content = picker.viewport, picker.rows_height
local function snap(v) return math.floor(v / P + 0.5) * P end
local length = math.min(viewport, math.max(9 * P, snap(viewport * viewport / content)))
local travel = viewport - length
local expected_top = snap(travel * picker.s / (content - viewport))
local tx, ty, tw, th = quad_at(quads_in(picker.scroll_bar.group)[2])
local px, py, pw, ph = panel_rect()
eq(ty - (py + 3 * P + 16 * P), expected_top)
eq(th, length)
"#,
            r#"
local picker = bar.picker
expect(picker ~= nil, "a press on the track leaves the picker open")
eq(picker.s, picker.max_s, "a press below the thumb moved a viewport, here to the end")
eq(list:scroll_position(), 0)
"#,
        ],
    );
    rig.opened();
    let thumb = point(&rig, "thumb");
    wheel(&mut rig, thumb, -1);
    rig.opened();
    let thumb = point(&rig, "thumb");
    rig.feed(&[
        PointerEvent::Down(points(thumb.0, thumb.1)),
        PointerEvent::Move(points(thumb.0, thumb.1 + 20.0)),
        PointerEvent::Up(points(thumb.0, thumb.1 + 20.0)),
    ]);
    frame(&mut rig);
    rig.opened();
    let track = point(&rig, "track");
    click_at(&mut rig, track);
    rig.opened();
}

#[test]
fn ticking_a_box_of_an_overflowing_picker_keeps_the_tick_and_when_the_boxes_fit_the_bar_goes_and_the_ticks_stay()
 {
    let mut rig = rig(
        &bar_body(12),
        &[
            r"
bar.filter_button.on_click()
picker_marks()
",
            r#"
expect(bar.picker ~= nil, "a tick leaves the picker open")
eq(bar.ticks, 1, "the box is ticked")
expect(bar.ticked.kind["k01"] == true, "the first box")
eq(#quads_in(bar.picker.scroll_bar.group), 2, "and the bar is still there")
-- Fewer boxes: the picker is made again, and the ticked box is kept besides.
local old = bar.picker.scroll_bar
bar:view(kinds(3))
local picker = bar.picker
eq(bar_hits(old), 0, "the bar is gone")
eq(bar_hits(picker.scroll_bar), 0)
local x, y, w, h = panel_rect()
eq(w, picker.content_width + 8 * P, "the panel is its old width")
eq(bar.ticks, 1, "the tick is kept")
expect(bar.ticked.kind["k01"] == true, "the same box")
"#,
        ],
    );
    rig.opened();
    // The first box row of the open picker, over the rows.
    let row = point(&rig, "row1");
    click_at(&mut rig, row);
    rig.opened();
}

#[test]
fn a_press_on_the_bar_does_not_close_the_picker_and_everything_else_closes_it_as_before() {
    let mut rig = rig(
        &bar_body(12),
        &[r#"
bar.filter_button.on_click()
local sb = bar.picker.scroll_bar
local key = bar.picker.scroll_bar.thumb_key
local track = bar.picker.scroll_bar.track_key
local function press(on) lb.update({pointer = {events = {{kind = "down", key = on}}}}) end
press(key)
expect(bar.picker ~= nil, "a press on the thumb")
press(track)
expect(bar.picker ~= nil, "a press on the track")
press("elsewhere_1")
expect(bar.picker == nil, "a press elsewhere closes it")
bar.filter_button.on_click()
bar.filter_button.on_click()
expect(bar.picker == nil, "its own button closes it")
bar.filter_button.on_click()
lb.update({pointer = {events = {{kind = "map_down"}}}})
expect(bar.picker == nil, "a press on the map closes it")
bar.filter_button.on_click()
expect(lb.close_open(), "Escape closes it")
eq(bar_hits(sb), 0, "and the bar's regions go with it")
"#],
    );
    rig.opened();
}

#[test]
fn a_sort_picker_too_tall_for_its_room_has_the_bar_too() {
    let mut rig = rig(
        r#"
new_window(120, 100)
list = lists.new{window = W, x = 0, y = 0, width = 120 * P, rows = 2, items = {"1", "2"}}
local sorts = {}
for index = 1, 12 do sorts[index] = {key = "s" .. index, label = "sort " .. index, first = "ascending"} end
new_bar({sorts = sorts})
"#,
        &[r#"
bar.sort_button.on_click()
local picker = bar.picker
local ox, oy = content_origin()
local x, y, w, h = hit_rect(picker.panel_key)
eq(picker.kind, "sort")
eq(w, picker.content_width + 8 * P + 5 * P, "5 UI pixels wider")
eq(#quads_in(picker.scroll_bar.group), 2, "with the bar")
local tx, ty, tw, th = quad_at(quads_in(picker.scroll_bar.group)[1])
eq(ty, y + 3 * P, "a sort picker has no all button: the viewport starts under the panel's top margin")
eq(th, h - 6 * P)
eq(tx, x + w - 4 * P)
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------
// Open and close (rule 14)
// ---------------------------------------------------------------------------------------------

const WITH_LIST: &str = r#"
new_window(120, 100)
selected = nil
list = lists.new{window = W, x = 0, y = 0, width = 120 * P, rows = 2, items = {"1", "2", "3", "4"},
  on_select = function(index) selected = index end}
new_bar()
"#;

fn list_marks() -> String {
    format!(
        r#"{MARKS}
function list_marks()
  local x, y, w, h = hit_rect(list.row_keys[1])
  mark("list_row", x + 10, y + h / 2)
end
"#
    )
}

#[test]
fn the_filter_button_opens_and_closes_its_picker_and_the_sort_button_replaces_it() {
    let mut rig = rig(
        &format!("{WITH_LIST}\n{}", list_marks()),
        &[
            r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
eq(bar.picker, nil, "no picker is open at the start")
marks()
"#,
            r#"
expect(bar.picker ~= nil and bar.picker.kind == "filter", "pressing the filter button opens its picker")
eq(count_hits("listbar_box"), 2)
"#,
            r#"
eq(bar.picker, nil, "pressing it again closes it")
eq(count_hits("listbar_box"), 0)
"#,
            r#"
expect(bar.picker ~= nil and bar.picker.kind == "filter")
"#,
            r#"
expect(bar.picker ~= nil and bar.picker.kind == "sort", "pressing the sort button with the filter picker open leaves only the sort picker open")
eq(count_hits("listbar_box"), 0)
eq(count_hits("listbar_sort"), 3)
expect(lb.close_open() == true)
eq(bar.picker, nil)
expect(lb.close_open() == false, "nothing is open")
"#,
        ],
    );
    rig.opened();
    click(&mut rig, "filter_button");
    rig.opened();
    click(&mut rig, "filter_button");
    rig.opened();
    click(&mut rig, "filter_button");
    rig.opened();
    click(&mut rig, "sort_button");
    rig.opened();
}

#[test]
fn a_map_down_and_a_press_on_another_key_close_the_picker_and_the_press_still_happens_a_press_on_a_box_does_not()
 {
    let mut rig = rig(
        &format!("{WITH_LIST}\n{}", list_marks()),
        &[
            r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
bar.filter_button.on_click()
marks() list_marks()
"#,
            r#"
expect(bar.picker ~= nil, "a press on a box keeps the picker open")
eq(bar.ticks, 1)
marks() list_marks()
"#,
            r#"
eq(bar.picker, nil, "a map_down closes it")
bar.filter_button.on_click()
marks() list_marks()
"#,
            r#"
eq(bar.picker, nil, "a down on a list row closes it")
eq(selected, 1, "and the row's click still happens")
bar.filter_button.on_click()
marks() list_marks()
"#,
            r#"
expect(bar.picker ~= nil, "a press on the panel's blank part keeps it open")
"#,
        ],
    );
    rig.opened();
    click(&mut rig, "row1");
    rig.opened();
    click(&mut rig, "elsewhere");
    rig.opened();
    click(&mut rig, "list_row");
    rig.opened();
    click(&mut rig, "panel_corner");
    rig.opened();
}

#[test]
fn the_events_of_an_overflow_packet_a_new_window_size_and_a_second_bars_picker_close_or_replace_as_ruled()
 {
    let mut rig = rig(
        &format!("{WITH_LIST}\n{}", list_marks()),
        &[
            r#"
bar:view({row("a", "ringfort", 1)})
bar.filter_button.on_click()
lb.update{width = 800, height = 600, pointer = {overflow = true, events = {{kind = "map_down"}, {kind = "down", key = "list_row_1"}}}}
expect(bar.picker ~= nil, "the events of an overflow packet close nothing")
lb.update{width = 800, height = 600, pointer = {events = {{kind = "down", key = bar.picker.panel_key}}}}
expect(bar.picker ~= nil, "a press on the picker's own key does not")
lb.update{width = 800, height = 600, pointer = {events = {{kind = "down", key = bar.filter_button.key}}}}
expect(bar.picker ~= nil, "nor does one on the button that opened it")
lb.update{width = 800, height = 600, pointer = {events = {{kind = "move"}, {kind = "up", key = "somewhere"}}}}
expect(bar.picker ~= nil, "a move and a release close nothing")
lb.update{width = 800, height = 600, pointer = {events = {{kind = "down", key = bar.sort_button.key}}}}
eq(bar.picker, nil, "a press on the other button does")
-- Only one picker is open in the whole UI.
local second = windows.new{title = "other", content_width = 120 * P, content_height = 100 * P, x = 300, y = 20, kind = "place"}
other = lb.new{window = second, sorts = SORTS, groups = GROUPS}
other:view({row("z", "ringfort", 1)})
bar.filter_button.on_click()
expect(bar.picker ~= nil)
other.sort_button.on_click()
eq(bar.picker, nil, "opening one closes the other, in another window too")
expect(other.picker ~= nil and other.picker.kind == "sort")
expect(lb.close_open())
bar.filter_button.on_click()
"#,
            r#"
expect(bar.picker ~= nil, "the picker is open for the resize")
"#,
            r#"
eq(bar.picker, nil, "a change of the game window's size closes it")
"#,
        ],
    );
    rig.opened();
    rig.opened();
    rig.deliver_fixture_frame_at(900, 600, 2);
    rig.opened();
}

#[test]
fn closing_the_window_or_destroying_the_bar_with_a_picker_open_leaves_none_of_its_keys_registered()
{
    let mut rig = rig(
        WITH_LIST,
        &[r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
bar.filter_button.on_click()
keys = {}
for key in pairs(bar.picker.owned) do keys[#keys + 1] = key end
eq(#keys, 4, "the panel, two boxes and the all button")
local before = count_hits("listbar_")
bar:destroy()
eq(bar.picker, nil)
for _, key in ipairs(keys) do
  eq(pointer_lib.widget(key), nil, "unregistered " .. key)
  eq(scene.hits[key], nil, "no region for " .. key)
end
expect(lb.close_open() == false)
-- Again, closing the window.
new_window(120, 100)
new_bar()
bar:view({row("a", "ringfort", 1)})
bar.sort_button.on_click()
keys = {}
for key in pairs(bar.picker.owned) do keys[#keys + 1] = key end
W:close()
for _, key in ipairs(keys) do
  eq(pointer_lib.widget(key), nil, "unregistered after the window closed: " .. key)
  eq(scene.hits[key], nil, "no region after the window closed: " .. key)
end
expect(lb.close_open() == false, "no picker is open")
eq(count_hits("listbar_"), 0)
"#],
    );
    rig.opened();
}

#[test]
fn every_key_the_bar_makes_is_a_short_fixed_prefix_whatever_the_labels_values_and_groups_say() {
    // CK-270's colon key crashed the game: `render.set_hit` takes [a-z0-9_]{1,48}. The real host call is in the rig, so a key built from
    // these odd strings would raise.
    let mut rig = rig(
        r#"
new_window(160, 120)
new_bar{
  sorts = {{key = "base price", label = "buy/sell: base price", first = "ascending"}, {key = "x:y", label = "\u{D3}\u{D3}", first = "descending"}},
  groups = {{key = "kind:of", label = "Kind Of: Place"}, {key = "tag/s", label = string.rep("w", 90)}},
}
"#,
        &[r#"
local long = string.rep("v", 120) .. ":" .. "\u{D3}"
local rows = {
  {name = "a", sort = {["base price"] = 1}, filter = {["kind:of"] = long, ["tag/s"] = {"A B", "c:d"}}},
  {name = "b", sort = {["base price"] = 2}, filter = {["kind:of"] = "northern_ui_neill", ["tag/s"] = "e/f"}},
}
bar:view(rows)
bar.filter_button.on_click()
for key in pairs(bar.picker.owned) do
  expect(key:match("^[a-z0-9_]+$") ~= nil and #key <= 48, "a valid key: " .. key)
  expect(key:match("^listbar_") ~= nil or key:match("^button_") ~= nil, "a fixed prefix: " .. key)
end
bar:toggle(1, long)
bar.sort_button.on_click()
for key in pairs(bar.picker.owned) do
  expect(key:match("^[a-z0-9_]+$") ~= nil and #key <= 48, "a valid key: " .. key)
end
bar:choose(2)
eq(#bar:view(rows), 1)
"#],
    );
    rig.opened();
}

#[test]
fn pressing_a_retained_box_or_all_that_rebuilds_the_picker_keeps_it_open_through_the_real_dispatch()
{
    // CK-295 review F1: the press's action refreshes the rows (as the ledger's `on_change` does), which makes the picker again with
    // new keys; the same packet's `down` names an old key, which is not an outside press.
    let mut rig = rig(
        &format!(
            r"
new_window(120, 100)
rows = {{}}
new_bar{{on_change = function() changes = changes + 1; bar:view(rows) end}}
{MARKS}
"
        ),
        &[
            r#"
rows = {row("a", "ringfort", 1), row("b", "crannog", 2)}
bar:view(rows)
bar.filter_button.on_click()
bar:toggle(1, "crannog")
-- The crannog leaves the rows; its box stays while it is ticked.
rows = {row("a", "ringfort", 1)}
bar:view(rows)
eq(#bar.picker.entries, 2, "the retained box is still there")
marks()
"#,
            r#"
eq(bar.ticks, 0, "the retained box was unticked")
expect(bar.picker ~= nil and not bar.picker.closed, "and the picker stays open")
eq(#bar.picker.entries, 1, "with only the ringfort box left")
-- "all" clearing a retained value does the same.
bar:toggle(1, "crannog") -- ticked though no row has it: on_change views the rows, and the box is there
eq(#bar.picker.entries, 2)
local x, y, w, h = hit_rect(bar.picker.all.key)
mark("all", x + w / 2, y + h / 2)
"#,
            r#"
eq(bar.ticks, 0, "all unticked the retained value")
expect(bar.picker ~= nil and not bar.picker.closed, "and the picker stays open")
eq(#bar.picker.entries, 1)
"#,
        ],
    );
    rig.opened();
    click(&mut rig, "row1");
    rig.opened();
    click(&mut rig, "all");
    rig.opened();
}
