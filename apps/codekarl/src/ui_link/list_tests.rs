//! The scroll list (`scripts/ui/lib/list.lua`, milestone 8, slice 6) in a real UI VM over the real `render` API, a real scene bridge
//! with a recording sink, `ck-input`'s `Mapper` and the real routing: geometry, validation, the scroll formulas, the stationary
//! viewport hit, row keys, clicks, the window's attach seam, and the wheel never reaching the map.
//!
//! A script writes what it saw with `window.record("name=value")`; the tests read those lines and the published scene. Nothing sleeps:
//! every callback and every update is awaited.

use ck_lua::ConfigValue;
use ck_window::{PointerEvent, ScrollUnit};

#[path = "../../tests/support/places_list.rs"]
mod places_list;

use super::group_tests::Rig;
use super::widget_witness_tests::points;
use crate::widget_tests::time_bar_witness as time_bar;

/// The header of every script: the libraries, helpers, and one `frame_update` that dispatches the pointer and records the list's
/// state. `lst` is the list under test (a global, so later callbacks reach it).
const HEADER: &str = r#"
local ui = require("lib.ui")
windows = require("lib.window")
lists = require("lib.list")
pointer = require("lib.pointer")
P = ui.P
H = lists.ROW
function names(count)
  local t = {}
  for i = 1, count do t[i] = "Item " .. i end
  return t
end
-- A routed event as the router gives it.
function ev(kind, key, x, y, inside)
  return {kind = kind, key = key, x = x, y = y, inside = inside ~= false}
end
function note(text) window.record(text) end
function dispatch(events, over, overflow)
  return pointer.dispatch({x = 0, y = 0, held = false, over = over or false, overflow = overflow or false, events = events})
end
selected = {}
frame.on_update(function(f)
  local kinds = {}
  if f.pointer ~= nil then
    for _, e in ipairs(f.pointer.events) do
      kinds[#kinds + 1] = e.kind .. ":" .. tostring(e.key) .. ":" .. tostring(e.inside)
    end
  end
  pointer.dispatch(f.pointer)
  if #kinds > 0 then
    note("events=" .. table.concat(kinds, ","))
  end
  if lst ~= nil and not lst.closed then
    note(string.format("update s=%g pos=%g", lst.s, lst.pos))
  end
end)
"#;

/// The lines the script recorded, in order.
fn notes(rig: &Rig) -> Vec<String> {
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

/// The line `name=value` recorded last, as `value`.
fn note_of(rig: &Rig, name: &str) -> String {
    let prefix = format!("{name}=");
    notes(rig)
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
        .unwrap_or_else(|| panic!("no note `{name}`: {:?}", notes(rig)))
}

type Rect = (f64, f64, f64, f64);

/// The quads of the published frame that carry `clip`, as (x, y, width, height): a clipped group's quads, translated by its offset.
fn quads_clipped_to(rig: &Rig, clip: Rect) -> Vec<Rect> {
    rig.sink
        .quads()
        .iter()
        .filter(|quad| {
            quad.clip.is_some_and(|c| {
                (
                    c.x.to_bits(),
                    c.y.to_bits(),
                    c.width.to_bits(),
                    c.height.to_bits(),
                ) == (
                    clip.0.to_bits(),
                    clip.1.to_bits(),
                    clip.2.to_bits(),
                    clip.3.to_bits(),
                )
            })
        })
        .map(|quad| (quad.x, quad.y, quad.width, quad.height))
        .collect()
}

/// A scroll event at (`x`, `y`) of `amount` units (lines), in the pointer's 1/256 fixed point.
fn scroll_lines(x: f64, y: f64, amount: i32) -> PointerEvent {
    PointerEvent::Scroll {
        position: points(x, y),
        amount: amount * 256,
        unit: ScrollUnit::Lines,
    }
}

fn scroll_points(x: f64, y: f64, amount: i32) -> PointerEvent {
    PointerEvent::Scroll {
        position: points(x, y),
        amount: amount * 256,
        unit: ScrollUnit::Points,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------------------------------------

/// A window at (100, 50) with a 100-point-wide list of 5 rows over 20 items, without `on_select`.
fn geometry_script(width: u32, content: u32) -> String {
    format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = {content}, content_height = {content}}}
lst = lists.new{{window = w, x = 0, y = 0, width = {width}, rows = 5, items = names(20)}}
window.on_opened(function()
  note("base=" .. lst.base)
  note("max_s=" .. lst.max_s)
  note("view=" .. lst.view_height)
  note("row=" .. H)
end)
"#
    )
}

#[test]
fn the_geometry_of_a_list_is_literal_at_two_points_per_pixel() {
    let mut rig = Rig::new_fixture_body(&geometry_script(100, 200));
    rig.opened();
    // A row is (11 + 2) * 2 = 26 points; 5 rows are 130; 20 items are 520 and scroll by at most 15 rows = 390.
    assert_eq!(note_of(&rig, "row"), "26");
    assert_eq!(note_of(&rig, "view"), "130");
    assert_eq!(note_of(&rig, "max_s"), "390");
    // The window at (100, 50): title 30 high and 6 of padding put the content origin at (106, 86). The rows group sits there, and its
    // clip is the list's visible rectangle (the content area is larger).
    let rows = quads_clipped_to(&rig, (106.0, 86.0, 100.0, 130.0));
    assert!(
        !rows.is_empty(),
        "the rows group is clipped to the visible rectangle"
    );
    // Row 1's glyph is one UI pixel in and one down: local (2, 2) plus the offset (106, 86).
    assert!(
        rows.iter().any(|rect| (rect.0, rect.1) == (108.0, 88.0)),
        "{rows:?}"
    );
    // The viewport hit covers the visible rectangle in the content group and nothing around it.
    let base = note_of(&rig, "base");
    let area = format!("{base}_area");
    assert_eq!(rig.key_at(106.5, 86.5).as_deref(), Some(area.as_str()));
    assert_eq!(rig.key_at(205.5, 215.5).as_deref(), Some(area.as_str()));
    assert_ne!(rig.key_at(206.5, 100.0).as_deref(), Some(area.as_str()));
    assert_ne!(rig.key_at(150.0, 216.5).as_deref(), Some(area.as_str()));
}

#[test]
fn column_rows_align_right_cells_and_keep_string_rows_unchanged() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Columns", x = 100, y = 50, content_width = 100, content_height = 130}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = {{
  {{cells = {{{{text = "left", x = 5, align = "left"}}, {{text = "42", x = 30, align = "right"}}}}}},
  "plain",
}}}}
local right = lst.lines[1].lines[2]
note(string.format("right-end %g", right.x + right.width * P))
window.on_opened(function() end)
"#
    );
    let rig = Rig::new(&script);
    let text_quads = quads_clipped_to(&rig, (106.0, 86.0, 100.0, 130.0));
    assert!(
        text_quads
            .iter()
            .any(|quad| (quad.0 - 116.0).abs() < 0.001 && (quad.1 - 88.0).abs() < 0.001),
        "left text starts at the requested five UI pixels: {text_quads:?}"
    );
    assert!(
        notes(&rig).contains(&"right-end 60".to_owned()),
        "right cell ends at x=30 UI pixels in the window-local coordinate space: {:?}",
        notes(&rig)
    );
    assert!(
        text_quads
            .iter()
            .any(|quad| (quad.0 - 108.0).abs() < 0.001 && (quad.1 - 114.0).abs() < 0.001),
        "string rows keep the existing one-pixel inset on row 2: {text_quads:?}"
    );
}

#[test]
fn tooltip_only_lists_register_passive_row_keys_without_selection_look() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Tooltips", x = 100, y = 50, content_width = 100, content_height = 240}}
local tooltip = require("lib.tooltip")
local text = require("lib.text")
local real_set = text.set
text.set = function(line, value) real_set(line, value); note("tip=" .. value) end
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 2, tooltips = true, items = {{
  {{cells = {{{{text = "one", x = 1, align = "left"}}}}, tooltip = function() return {{"first tip"}} end}},
  {{cells = {{{{text = "two", x = 1, align = "left"}}}}, tooltip = function() return {{"second tip"}} end}},
  "third", "fourth",
}}}}
plain = lists.new{{window = w, x = 0, y = 40, width = 100, rows = 2, items = names(2)}}
string_list = lists.new{{window = w, x = 0, y = 80, width = 100, rows = 1, tooltips = true, items = {{"plain"}}}}
local bad, bad_error = pcall(function()
  lists.new{{window = w, x = 0, y = 120, width = 100, rows = 1, tooltips = true,
    items = {{{{cells = {{{{text = "bad", x = 1, align = "left"}}}}, tooltip = 12}}}}}}
end)
window.on_opened(function()
  note("key1=" .. tostring(lst.row_keys[1]))
  note("key2=" .. tostring(lst.row_keys[2]))
  note("key3=" .. tostring(lst.row_keys[3]))
  note("key4=" .. tostring(lst.row_keys[4]))
  note("functional=" .. tostring(pointer.functional(lst.row_keys[1])))
  note("functional_member_missing=" .. tostring(pointer.widget(lst.row_keys[1]).functional == nil))
  note("passive_noop_methods=" .. tostring(type(pointer.widget(lst.row_keys[1]).on_hover) == "function"
    and type(pointer.widget(lst.row_keys[1]).on_overflow) == "function"))
  note("plain_key=" .. tostring(plain.row_keys[1]))
  note("string_key=" .. tostring(string_list.row_keys[1]))
  note("bad_error=" .. tostring(not bad and bad_error:find("item 1", 1, true) ~= nil))
end)
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
  note("row_state=" .. tostring(lst.hover_row) .. "/" .. tostring(lst.pressed_row))
  note("position=" .. tostring(lst.pos))
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    rig.opened();

    let first = note_of(&rig, "key1");
    assert!(first.ends_with("_row_1"), "{first}");
    assert!(note_of(&rig, "key2").ends_with("_row_2"));
    assert!(note_of(&rig, "key3") == "nil" || note_of(&rig, "key3").ends_with("_row_3"));
    assert_eq!(note_of(&rig, "key4"), "nil");
    assert_eq!(note_of(&rig, "functional"), "false");
    assert_eq!(note_of(&rig, "functional_member_missing"), "true");
    assert_eq!(note_of(&rig, "passive_noop_methods"), "true");
    assert_eq!(note_of(&rig, "plain_key"), "nil");
    let string_key = note_of(&rig, "string_key");
    assert!(string_key.ends_with("_row_1"));
    assert_eq!(note_of(&rig, "bad_error"), "true");
    assert_eq!(rig.key_at(107.0, 89.0).as_deref(), Some(first.as_str()));
    rig.feed(&[PointerEvent::Move(points(107.0, 89.0))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame();
    }
    assert!(notes(&rig).iter().any(|line| line == "tip=first tip"));
    assert_eq!(note_of(&rig, "row_state"), "nil/nil");
    rig.feed(&[
        PointerEvent::Down(points(107.0, 89.0)),
        PointerEvent::Up(points(107.0, 89.0)),
    ]);
    rig.deliver_fixture_frame();
    assert_eq!(note_of(&rig, "row_state"), "nil/nil");
    assert!(!notes(&rig).iter().any(|line| line == "selected"));
    rig.feed(&[scroll_lines(107.0, 89.0, -1)]);
    rig.deliver_fixture_frame();
    assert_ne!(note_of(&rig, "position"), "0");
    let string_point = (50..350)
        .find_map(|y| {
            (rig.key_at(107.0, f64::from(y)).as_deref() == Some(string_key.as_str()))
                .then_some((107.0, f64::from(y)))
        })
        .expect("the string row has a hit rectangle");
    let before_string = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(string_point.0, string_point.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame();
    }
    assert!(
        !notes(&rig)[before_string..]
            .iter()
            .any(|line| line.starts_with("tip=")),
        "a string item has no tooltip"
    );
}

#[test]
fn tooltip_sources_follow_changed_items_and_removed_rows_retire_their_keys() {
    let script = format!(
        r#"{HEADER}
local tooltip = require("lib.tooltip")
local text = require("lib.text")
local real_set = text.set
text.set = function(line, value) real_set(line, value); note("text=" .. value) end
local attached, attach_count, detach_count = {{}}, 0, 0
local real_attach, real_detach = tooltip.attach, tooltip.detach
tooltip.attach = function(key, source) attached[key] = source; attach_count = attach_count + 1; return real_attach(key, source) end
tooltip.detach = function(key) attached[key] = nil; detach_count = detach_count + 1; return real_detach(key) end
w = windows.new{{title = "Tips", x = 100, y = 50, content_width = 140, content_height = 240}}
local function item(label)
  return {{cells = {{{{text = label, x = 1, align = "left"}}}}, tooltip = function() return {{label}} end}}
end
lst = lists.new{{window = w, x = 0, y = 0, width = 140, rows = 3, tooltips = true,
  items = {{item("first"), item("second"), item("old"), item("fourth"), item("fifth"), item("sixth")}}}}
local selectable = lists.new{{window = w, x = 0, y = 80, width = 140, rows = 1, tooltips = true, items = {{item("selected")}}, on_select = function() note("selected") end}}
local third_key, frames, replaced = lst.row_keys[3], 0, false
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  frames = frames + 1
  tooltip.update(update)
  if frames == 70 and not replaced then
    lst:set_items({{item("first"), item("second"), item("new"), item("fourth"), item("fifth"), item("sixth")}})
    tooltip.refresh()
    replaced = true
    note("same_key=" .. tostring(third_key == lst.row_keys[3]))
    note("attachment_counts=" .. attach_count .. "/" .. detach_count)
  end
  if frames == 71 then
    pointer.widget(third_key):on_event({{kind = "scroll", amount = -3, unit = "lines"}})
    pointer.dispatch({{x = 0, y = 0, held = false, over = false, overflow = false, events = {{}}}})
  end
  note("old_widget=" .. tostring(pointer.widget(third_key) ~= nil))
  note("row_state=" .. tostring(lst.hover_row) .. "/" .. tostring(lst.pressed_row))
  note("position=" .. tostring(lst.pos))
end)
window.on_opened(function() note("key3=" .. lst.row_keys[3]); note("selectable_key=" .. selectable.row_keys[1]) end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    rig.opened();
    let third = note_of(&rig, "key3");
    assert!(
        (50..250).any(|y| rig.key_at(107.0, f64::from(y)).as_deref() == Some(third.as_str())),
        "the third row is a hit target"
    );
    let third_point = (50..250)
        .find_map(|y| {
            (rig.key_at(107.0, f64::from(y)).as_deref() == Some(third.as_str()))
                .then_some((107.0, f64::from(y)))
        })
        .unwrap();
    rig.feed(&[PointerEvent::Move(points(third_point.0, third_point.1))]);
    for _ in 0..69 {
        rig.deliver_fixture_frame();
    }
    rig.feed(&[PointerEvent::ButtonDown {
        position: points(third_point.0, third_point.1),
        button: ck_window::PointerButton::Middle,
    }]);
    rig.deliver_fixture_frame();
    let updated = notes(&rig);
    assert!(updated.iter().any(|line| line == "text=old"));
    assert!(updated.iter().any(|line| line == "text=new"));
    assert!(updated.iter().any(|line| line == "same_key=true"));
    assert!(
        updated.iter().any(|line| line == "attachment_counts=4/0"),
        "{updated:?}"
    );
    assert!(updated.iter().any(|line| line == "row_state=nil/nil"));
    rig.deliver_fixture_frame();
    let retired = notes(&rig);
    assert!(
        retired.iter().any(|line| line == "position=78"),
        "{retired:?}"
    );
    assert!(retired.iter().any(|line| line == "old_widget=false"));
    assert_ne!(
        rig.key_at(third_point.0, third_point.1).as_deref(),
        Some(third.as_str())
    );
    let next_row = rig
        .key_at(107.0, 89.0)
        .expect("the next item occupies the old row position");
    assert!(next_row.ends_with("_row_4"));
    let before_next = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(107.0, 89.0))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame();
    }
    assert!(
        notes(&rig)[before_next..]
            .iter()
            .any(|line| line == "text=fourth"),
        "the row now under the pointer owns its tooltip"
    );
}

#[test]
fn selectable_tooltip_rows_keep_their_hover_press_and_click_behavior() {
    let script = format!(
        r#"{HEADER}
local tooltip = require("lib.tooltip")
local text = require("lib.text")
local real_set = text.set
text.set = function(line, value) real_set(line, value); note("tip=" .. value) end
w = windows.new{{title = "Selectable", x = 100, y = 50, content_width = 140, content_height = 80}}
local item = {{cells = {{{{text = "chosen", x = 1, align = "left"}}}}, tooltip = function() return {{"selected tip"}} end}}
lst = lists.new{{window = w, x = 0, y = 0, width = 140, rows = 1, tooltips = true, items = {{item}}, on_select = function() note("selected") end}}
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
  note("selectable_state=" .. tostring(lst.hover_row) .. "/" .. tostring(lst.pressed_row))
end)
window.on_opened(function() note("selectable_key=" .. lst.row_keys[1]); note("selectable_functional=" .. tostring(pointer.functional(lst.row_keys[1]))) end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    rig.opened();
    let key = note_of(&rig, "selectable_key");
    assert_eq!(note_of(&rig, "selectable_functional"), "true");
    let point = (50..250)
        .find_map(|y| {
            (rig.key_at(107.0, f64::from(y)).as_deref() == Some(key.as_str()))
                .then_some((107.0, f64::from(y)))
        })
        .expect("the selectable row has a hit rectangle");
    rig.feed(&[PointerEvent::Move(points(point.0, point.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame();
    }
    assert!(notes(&rig).iter().any(|line| line == "tip=selected tip"));
    assert_eq!(note_of(&rig, "selectable_state"), "1/nil");
    rig.feed(&[PointerEvent::Down(points(point.0, point.1))]);
    rig.deliver_fixture_frame();
    assert_eq!(note_of(&rig, "selectable_state"), "1/1");
    rig.feed(&[PointerEvent::Up(points(point.0, point.1))]);
    rig.deliver_fixture_frame();
    assert!(notes(&rig).iter().any(|line| line == "selected"));
    assert_eq!(note_of(&rig, "selectable_state"), "1/nil");
}

#[test]
fn destroying_a_tooltip_list_detaches_live_and_retired_row_keys() {
    let script = format!(
        r#"{HEADER}
local tooltip = require("lib.tooltip")
local attached, detaches = {{}}, 0
local real_attach, real_detach = tooltip.attach, tooltip.detach
tooltip.attach = function(key, source) attached[key] = source; return real_attach(key, source) end
tooltip.detach = function(key) attached[key] = nil; detaches = detaches + 1; return real_detach(key) end
w = windows.new{{title = "Destroy", x = 100, y = 50, content_width = 140, content_height = 100}}
local function item(label)
  return {{cells = {{{{text = label, x = 1, align = "left"}}}}, tooltip = function() return {{label}} end}}
end
lst = lists.new{{window = w, x = 0, y = 0, width = 140, rows = 2, tooltips = true,
  items = {{item("one"), item("two"), item("three"), item("four")}}}}
local keys = {{lst.base .. "_row_1", lst.base .. "_row_2", lst.base .. "_row_3", lst.base .. "_row_4"}}
local destroyed = false
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
  local middle = false
  for _, event in ipairs(update.pointer and update.pointer.events or {{}}) do
    if event.kind == "button_down" and event.button == "middle" then middle = true end
  end
  if middle and not destroyed then
    lst:set_pos(26)
    note("retired_before_destroy=" .. tostring(pointer.widget(keys[1]) ~= nil))
    note("detached_on_scroll=" .. tostring(attached[keys[1]] == nil))
    lst:destroy()
    tooltip.refresh()
    destroyed = true
  local registered = 0
    for _, key in ipairs(keys) do
    if pointer.widget(key) ~= nil then registered = registered + 1 end
  end
  local sources = 0
  for _ in pairs(attached) do sources = sources + 1 end
  note("registered_after_destroy=" .. registered)
  note("sources_after_destroy=" .. sources)
  note("detach_count=" .. detaches)
    note("panel_after_refresh=" .. tostring(pointer.widget("tooltip_panel_1") ~= nil))
  end
end)
window.on_opened(function() note("key=" .. lst.row_keys[1]) end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    rig.opened();
    let key = note_of(&rig, "key");
    rig.feed(&[PointerEvent::Move(points(107.0, 89.0))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame();
    }
    rig.feed(&[PointerEvent::ButtonDown {
        button: ck_window::PointerButton::Middle,
        position: points(107.0, 89.0),
    }]);
    rig.deliver_fixture_frame();
    assert!(key.ends_with("_row_1"));
    assert_eq!(note_of(&rig, "retired_before_destroy"), "true");
    assert_eq!(note_of(&rig, "detached_on_scroll"), "true");
    assert_eq!(note_of(&rig, "registered_after_destroy"), "0");
    assert_eq!(note_of(&rig, "sources_after_destroy"), "0");
    assert_eq!(note_of(&rig, "detach_count"), "4");
    assert_eq!(note_of(&rig, "panel_after_refresh"), "false");
}

fn icon_reuse_script() -> String {
    format!(
        r##"{HEADER}
local ui = require("lib.ui")
local real_texture = ui.texture
local texture_names = {{}}
local active_icons = {{}}
local function icon_state()
  local count, name = 0, "-"
  for _, active_name in pairs(active_icons) do count = count + 1; name = active_name end
  return count, name
end
ui.texture = function(name)
  local token = real_texture(name)
  texture_names[token] = name
  return token
end
local real_create_quad = render.create_quad
render.create_quad = function(spec)
  local name = texture_names[spec.texture]
  local quad = real_create_quad(spec)
  if name ~= nil and (name:match("^ui/goods/") or name:match("^ui/theme/icon_")) then
    note(string.format("iconquad %s %g %g %g %g %s", name, spec.x, spec.y, spec.width, spec.height, tostring(spec.tint or "-")))
    active_icons[quad] = name
  end
  return quad
end
local real_destroy_quad = render.destroy_quad
render.destroy_quad = function(quad)
  local name = active_icons[quad]
  if name ~= nil then
    active_icons[quad] = nil
    local count = icon_state()
    note(string.format("icon destroy %s remaining %d", name, count))
  end
  return real_destroy_quad(quad)
end
w = windows.new{{title = "Goods", x = 100, y = 50, content_width = 100, content_height = 130}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = {{{{cells = {{{{text = "96.5 (100)", x = 16, align = "left"}}}}, icon = {{name = "location", colour = "#112233"}}}}}}}}
window.on_opened(function()
  local function replace(label, icon)
    lst:set_items({{{{cells = {{{{text = "value", x = 16, align = "left"}}}}, icon = icon}}}})
    local count, name = icon_state()
    note(string.format("icon state %s %d %s", label, count, name))
  end
  replace("oats", {{image = "goods/oats"}})
  replace("barley", {{image = "goods/barley"}})
  replace("theme", {{name = "location", colour = "#112233"}})
  replace("none", nil)
  note("no icon after removal " .. tostring(lst.lines[1].icon == nil))
  local function attempt(icon)
    local ok, err = pcall(lists.new, {{window = w, x = 0, y = 0, width = 100, rows = 1,
      items = {{{{cells = {{{{text = "x", x = 16, align = "left"}}}}, icon = icon}}}}}})
    note("invalid icon " .. tostring(ok) .. " " .. tostring(err):match("list%.new: [^\n]*"))
  end
  attempt({{name = "location", colour = "#ffffff", image = "goods/oats"}})
  attempt({{}})
end)
"##
    )
}

#[test]
fn image_icons_are_full_colour_centered_and_refresh_when_a_reused_row_changes_form() {
    let mut rig = Rig::new(&icon_reuse_script());
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "iconquad ui/goods/oats.png 8 4 18 18 -"),
        "image icon is 9x9 UI pixels at (4,2), untinted: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "iconquad ui/goods/barley.png 8 4 18 18 -"),
        "reused image rows load the replacement image: {recorded:?}"
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|line| line.starts_with("iconquad ui/goods/"))
            .count(),
        2,
        "the replacement image quad is created without a stale third image: {recorded:?}"
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|line| *line == "iconquad ui/theme/icon_location.png 8 2 18 22 #112233")
            .count(),
        2,
        "the existing theme icon and tint survive theme-to-image-to-theme reuse: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "icon state oats 1 ui/goods/oats.png"),
        "image replacement leaves exactly its one current quad: {recorded:?}"
    );
    for state in [
        "icon state barley 1 ui/goods/barley.png",
        "icon state theme 1 ui/theme/icon_location.png",
        "icon state none 0 -",
    ] {
        assert!(recorded.iter().any(|line| line == state), "{recorded:?}");
    }
    let destroyed = recorded
        .iter()
        .filter(|line| line.starts_with("icon destroy "))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        destroyed,
        [
            "icon destroy ui/theme/icon_location.png remaining 0",
            "icon destroy ui/goods/oats.png remaining 0",
            "icon destroy ui/goods/barley.png remaining 0",
            "icon destroy ui/theme/icon_location.png remaining 0",
        ],
        "each replacement removes the previous quad before leaving the row with its one next icon"
    );
    let refusals = recorded
        .iter()
        .filter(|line| line.contains("invalid icon false"))
        .collect::<Vec<_>>();
    assert_eq!(
        refusals.len(),
        2,
        "both malformed forms are refused: {recorded:?}"
    );
    assert!(
        refusals
            .iter()
            .all(|line| line.contains("name and colour") && line.contains("image"))
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------------------------------

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "each refused argument is listed with its message in one table"
)]
fn a_bad_argument_is_refused_with_a_message_and_creates_nothing() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
gone = windows.new{{title = "Gone", x = 10, y = 10, content_width = 50, content_height = 50}}
gone:close()
local good = {{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(3)}}
local function with(field, value)
  local spec = {{}}
  for k, v in pairs(good) do spec[k] = v end
  spec[field] = value
  return spec
end
window.on_opened(function()
  local cases = {{
    {{"rows 0", with("rows", 0)}},
    {{"rows 257", with("rows", 257)}},
    {{"rows 1.5", with("rows", 1.5)}},
    {{"rows nan", with("rows", 0/0)}},
    {{"rows text", with("rows", "5")}},
    {{"width 0", with("width", 0)}},
    {{"width -4", with("width", -4)}},
    {{"width inf", with("width", 1/0)}},
    {{"width nan", with("width", 0/0)}},
    {{"x nan", with("x", 0/0)}},
    {{"y inf", with("y", 1/0)}},
    {{"items number", with("items", 5)}},
    {{"items object", with("items", {{a = "b"}})}},
    {{"items mixed", with("items", {{"a", 2}})}},
    {{"items sparse", with("items", {{"a", nil, "c"}})}},
    {{"on_select number", with("on_select", 5)}},
    {{"window closed", with("window", gone)}},
    {{"window missing", with("window", nil)}},
    {{"window text", with("window", "w")}},
  }}
  for _, case in ipairs(cases) do
    local ok, err = pcall(lists.new, case[2])
    note(case[1] .. " -> " .. tostring(ok) .. " " .. (tostring(err):match("list%.new: [^\n]*") or tostring(err)))
  end
  local many = {{}}
  for i = 1, 65537 do many[i] = "x" end
  local ok, err = pcall(lists.new, with("items", many))
  note("items 65537 -> " .. tostring(ok) .. " " .. (tostring(err):match("list%.new: [^\n]*") or tostring(err)))
  -- The limits themselves are accepted.
  local edge = lists.new(with("rows", 256))
  note("rows 256 ok")
  edge:destroy()
  local single = lists.new(with("rows", 1))
  note("rows 1 ok")
  single:destroy()
  local narrow = lists.new(with("width", 0.4))
  note("width " .. narrow.width)
  narrow:destroy()
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    // The window's own quads, before any list but the one the script made at start-up has been refused or made.
    let published = rig.sink.count();
    let quads_before = rig.sink.quads().len();
    let regions_before = rig
        .harness
        .bridge
        .hit_regions()
        .expect("regions")
        .regions()
        .len();
    rig.opened();
    let lines = notes(&rig);
    let refused: Vec<&str> = lines
        .iter()
        .filter(|line| line.contains("-> false"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        refused,
        [
            "rows 0 -> false list.new: rows must be an integer from 1 to 256",
            "rows 257 -> false list.new: rows must be an integer from 1 to 256",
            "rows 1.5 -> false list.new: rows must be an integer from 1 to 256",
            "rows nan -> false list.new: rows must be an integer from 1 to 256",
            "rows text -> false list.new: rows must be an integer from 1 to 256",
            "width 0 -> false list.new: width must be a finite number greater than 0",
            "width -4 -> false list.new: width must be a finite number greater than 0",
            "width inf -> false list.new: width must be a finite number greater than 0",
            "width nan -> false list.new: width must be a finite number greater than 0",
            "x nan -> false list.new: x and y must be finite numbers",
            "y inf -> false list.new: x and y must be finite numbers",
            "items number -> false list.new: items must be an array of strings or rows",
            "items object -> false list.new: items must be an array of strings or rows",
            "items mixed -> false list.new: item 2 is not a string or row table",
            "items sparse -> false list.new: items must be an array of strings or rows",
            "on_select number -> false list.new: on_select must be nil or a function",
            "window closed -> false list.new: window must be an open window",
            "window missing -> false list.new: window must be an open window",
            "window text -> false list.new: window must be an open window",
            "items 65537 -> false list.new: at most 65536 items",
        ]
    );
    // Every refusal came before anything was created, and the accepted limits left nothing behind: the callback made and destroyed
    // three lists, and the scene and the regions are what they were. A width below one UI pixel became one.
    assert!(lines.contains(&"width 2".to_owned()), "{lines:?}");
    assert!(lines.contains(&"rows 256 ok".to_owned()) && lines.contains(&"rows 1 ok".to_owned()));
    assert!(
        rig.sink.count() > published,
        "the callback did publish its create/destroy"
    );
    assert_eq!(rig.sink.quads().len(), quads_before);
    assert_eq!(
        rig.harness
            .bridge
            .hit_regions()
            .expect("regions")
            .regions()
            .len(),
        regions_before
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// The scroll value
// ---------------------------------------------------------------------------------------------------------------------

/// A 3-row list over 100 items at 2 points per pixel, scripted in steps: each `rig.opened()` runs the next one.
fn steps_script(steps: &str) -> String {
    format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 3, items = names(100)}}
function sc(unit, amount)
  lst:on_event({{kind = "scroll", amount = amount, unit = unit}})
  note(string.format("pos=%g s=%g", lst.pos, lst.s))
end
local steps = {{
{steps}
}}
local n = 0
window.on_opened(function()
  n = n + 1
  steps[n]()
end)
"#
    )
}

#[test]
fn the_scroll_formulas_have_literal_positions_for_lines_and_points_at_two_points_per_pixel() {
    // h = 26. A positive amount scrolls toward earlier items (pos falls), a negative one toward later items.
    let steps = [
        ("lines", -1.0, "pos=26 s=26"),
        ("lines", 1.0, "pos=0 s=0"),
        ("lines", -0.5, "pos=13 s=14"),
        ("lines", 0.5, "pos=0 s=0"),
        ("points", -7.0, "pos=7 s=8"),
        ("points", 7.0, "pos=0 s=0"),
        ("points", -0.25, "pos=0.25 s=0"),
        ("points", 0.25, "pos=0 s=0"),
    ];
    let body = steps
        .iter()
        .map(|(unit, amount, _)| format!("function() sc('{unit}', {amount}) end,"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut rig = Rig::new_fixture_body(&steps_script(&body));
    for (index, (_, _, expected)) in steps.iter().enumerate() {
        rig.opened();
        let seen = notes(&rig);
        let last = seen
            .iter()
            .rev()
            .find(|line| line.starts_with("pos="))
            .unwrap();
        assert_eq!(last, expected, "step {}", index + 1);
    }
}

#[test]
fn fractional_remainders_carry_between_events_until_the_sum_crosses_the_snap() {
    let steps = (0..6)
        .map(|_| "function() sc('points', -0.75) end,")
        .collect::<Vec<_>>()
        .join("\n");
    let mut rig = Rig::new_fixture_body(&steps_script(&steps));
    let mut seen = Vec::new();
    for _ in 0..6 {
        rig.opened();
        let lines = notes(&rig);
        seen.push(
            lines
                .iter()
                .rev()
                .find(|line| line.starts_with("pos="))
                .unwrap()
                .clone(),
        );
    }
    // 0.75, 1.5, 2.25, 3.0, 3.75, 4.5 points: the displayed scroll moves by one UI pixel (2 points) when the sum first rounds to 1
    // (at 1.5: 0.75 is below the half), and by the next when it reaches 3.0.
    assert_eq!(
        seen,
        [
            "pos=0.75 s=0",
            "pos=1.5 s=2",
            "pos=2.25 s=2",
            "pos=3 s=4",
            "pos=3.75 s=4",
            "pos=4.5 s=4",
        ]
    );
}

#[test]
fn clamping_discards_the_excess_at_both_ends_and_an_unchanged_scroll_makes_no_scene_call() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 3, items = names(5)}}
function sc(unit, amount)
  lst:on_event({{kind = "scroll", amount = amount, unit = unit}})
  note(string.format("pos=%g s=%g", lst.pos, lst.s))
end
local steps = {{
  function() sc("points", 50) end,   -- past the top: nothing to discard from, pos stays 0
  function() sc("points", -10) end,  -- and back: from 0 at once
  function() sc("points", -200) end, -- past the end (max_s = 52): clamped
  function() sc("points", 10) end,   -- and back: from the end at once
  function() sc("points", -0.25) end,
}}
local n = 0
window.on_opened(function()
  n = n + 1
  steps[n]()
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    let step = |rig: &mut Rig| {
        let before = rig.sink.count();
        rig.opened();
        let lines = notes(rig);
        (
            lines
                .iter()
                .rev()
                .find(|line| line.starts_with("pos="))
                .unwrap()
                .clone(),
            rig.sink.count() - before,
        )
    };
    assert_eq!(
        step(&mut rig),
        ("pos=0 s=0".to_owned(), 0),
        "past the top: no change, no scene call"
    );
    assert_eq!(
        step(&mut rig),
        ("pos=10 s=10".to_owned(), 1),
        "from 0 at once"
    );
    assert_eq!(
        step(&mut rig),
        ("pos=52 s=52".to_owned(), 1),
        "clamped to max_s = (5 - 3) * 26"
    );
    assert_eq!(
        step(&mut rig),
        ("pos=42 s=42".to_owned(), 1),
        "from the end at once"
    );
    // A quarter point back toward the end: pos 42.25 snaps to the same 42, so no scene call.
    assert_eq!(step(&mut rig), ("pos=42.25 s=42".to_owned(), 0));
}

// ---------------------------------------------------------------------------------------------------------------------
// The stationary viewport hit and the wheel
// ---------------------------------------------------------------------------------------------------------------------

/// A list of `items` items and 5 rows at the window's content origin (106, 86 at 2 points per pixel), with or without `on_select`.
fn routed_script(items: u32, with_select: bool) -> String {
    let select = if with_select {
        ", on_select = function(i) selected[#selected + 1] = i; note('select ' .. i) end"
    } else {
        ""
    };
    format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names({items}){select}}}
note("base=" .. lst.base)
window.on_opened(function()
  note("base=" .. lst.base)
  note("keys=" .. (function() local n = 0 for _ in pairs(lst.row_keys) do n = n + 1 end return n end)())
end)
"#
    )
}

fn camera(rig: &Rig) -> Option<ck_map::Camera> {
    crate::map_app::lock(&rig.map).camera()
}

/// The last `update s=...` line.
fn displayed(rig: &Rig) -> String {
    notes(rig)
        .iter()
        .rev()
        .find(|line| line.starts_with("update "))
        .cloned()
        .unwrap_or_default()
}

#[test]
fn the_wheel_over_a_list_without_on_select_reaches_it_over_its_rows_and_scrolls_both_ways_and_never_zooms_the_map()
 {
    let mut rig = Rig::new_fixture_body(&routed_script(20, false));
    rig.opened();
    let area = format!("{}_area", note_of(&rig, "base"));
    let map = camera(&rig);
    // Over the rows area (no row has a key): the wheel one line toward later items, then back.
    rig.feed(&[scroll_lines(150.0, 100.0, -1)]);
    rig.deliver_fixture_frame();
    assert!(
        notes(&rig).contains(&format!("events=scroll:{area}:true")),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(displayed(&rig), "update s=26 pos=26");
    rig.feed(&[scroll_lines(150.0, 200.0, 1)]);
    rig.deliver_fixture_frame();
    assert_eq!(displayed(&rig), "update s=0 pos=0");
    // To the end, and back from it: the viewport is still under the pointer at every scroll position.
    rig.feed(&[scroll_lines(110.0, 90.0, -40)]);
    rig.deliver_fixture_frame();
    assert_eq!(displayed(&rig), "update s=390 pos=390");
    rig.feed(&[scroll_lines(205.0, 215.0, 2)]);
    rig.deliver_fixture_frame();
    assert_eq!(displayed(&rig), "update s=338 pos=338");
    assert_eq!(camera(&rig), map, "the map did not zoom");
}

#[test]
fn the_wheel_over_the_empty_part_of_a_list_reaches_it_with_and_without_on_select() {
    for with_select in [false, true] {
        // Three items in five rows: rows 4 and 5 are empty space inside the viewport.
        let mut rig = Rig::new_fixture_body(&routed_script(3, with_select));
        rig.opened();
        let area = format!("{}_area", note_of(&rig, "base"));
        let map = camera(&rig);
        // y = 86 + 3 * 26 = 164 is where the last row ends.
        assert_eq!(
            rig.key_at(150.0, 190.0).as_deref(),
            Some(area.as_str()),
            "select {with_select}"
        );
        rig.feed(&[scroll_lines(150.0, 190.0, -1)]);
        rig.deliver_fixture_frame();
        assert!(
            notes(&rig).contains(&format!("events=scroll:{area}:true")),
            "select {with_select}: {:?}",
            notes(&rig)
        );
        // Nothing to scroll (max_s is 0), but the wheel was the list's and not the map's.
        assert_eq!(displayed(&rig), "update s=0 pos=0");
        assert_eq!(
            camera(&rig),
            map,
            "select {with_select}: the map did not zoom"
        );
    }
}

#[test]
fn the_wheel_over_a_row_with_on_select_reaches_the_list_through_the_row_key() {
    let mut rig = Rig::new_fixture_body(&routed_script(20, true));
    rig.opened();
    let base = note_of(&rig, "base");
    let map = camera(&rig);
    // Row 2 is at y 112..138: its centre is (150, 125).
    assert_eq!(rig.key_at(150.0, 125.0), Some(format!("{base}_row_2")));
    rig.feed(&[scroll_points(150.0, 125.0, -30)]);
    rig.deliver_fixture_frame();
    assert!(
        notes(&rig).contains(&format!("events=scroll:{base}_row_2:true")),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(displayed(&rig), "update s=30 pos=30");
    assert_eq!(camera(&rig), map, "the map did not zoom");
    // With the map: a scroll outside the window's frame zooms it, so the checks above are not vacuous.
    rig.feed(&[scroll_lines(390.0, 300.0, 1)]);
    rig.deliver_fixture_frame();
    assert_ne!(
        camera(&rig),
        map,
        "a wheel away from every region zooms the map"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Row keys
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn only_the_rows_in_view_have_keys_at_most_rows_plus_one_and_scrolling_swaps_them_in_one_callback()
{
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(20), on_select = function() end}}
local function keys()
  local out = {{}}
  for i in pairs(lst.row_keys) do out[#out + 1] = i end
  table.sort(out)
  return table.concat(out, ",")
end
local steps = {{
  function() note("keys0=" .. keys()) end,
  function() lst:on_event({{kind = "scroll", amount = -13, unit = "points"}}); note("keys1=" .. keys()) end,
  function() lst:on_event({{kind = "scroll", amount = -2, unit = "lines"}}); note("keys2=" .. keys()) end,
  function() lst:set_pos(lst.max_s); note("keys3=" .. keys()) end,
}}
local n = 0
window.on_opened(function()
  n = n + 1
  steps[n]()
  note("base=" .. lst.base)
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    rig.opened();
    // At s = 0 rows 1 to 5 are in view, and only they have keys.
    assert_eq!(note_of(&rig, "keys0"), "1,2,3,4,5");
    let base = note_of(&rig, "base");
    for row in 1..=5 {
        let y = 86.0 + 26.0 * f64::from(row - 1) + 13.0;
        assert_eq!(rig.key_at(150.0, y), Some(format!("{base}_row_{row}")));
    }
    // A half row (13 points snaps to 14): row 1 is cut at the top and row 6 shows at the bottom, 6 = rows + 1 keys; one callback
    // published the offset and the key swap together, as one frame.
    let published = rig.sink.count();
    rig.opened();
    assert_eq!(rig.sink.count(), published + 1);
    assert_eq!(note_of(&rig, "keys1"), "1,2,3,4,5,6");
    assert_eq!(rig.key_at(150.0, 90.0), Some(format!("{base}_row_1")));
    assert_eq!(rig.key_at(150.0, 210.0), Some(format!("{base}_row_6")));
    // Two more rows (s = 14 + 52 = 66): rows 3 to 8 are in view.
    rig.opened();
    assert_eq!(note_of(&rig, "keys2"), "3,4,5,6,7,8");
    assert_ne!(rig.key_at(150.0, 90.0), Some(format!("{base}_row_1")));
    // At the end (s = 390): rows 16 to 20.
    rig.opened();
    assert_eq!(note_of(&rig, "keys3"), "16,17,18,19,20");
}

// ---------------------------------------------------------------------------------------------------------------------
// Selecting a row
// ---------------------------------------------------------------------------------------------------------------------

/// The highlight quads of the rows group (the only ones as wide as the list and as high as a row), hover first.
fn highlights(rig: &Rig) -> Vec<Rect> {
    quads_clipped_to(rig, (106.0, 86.0, 100.0, 130.0))
        .into_iter()
        .filter(|rect| (rect.2, rect.3) == (100.0, 26.0))
        .collect()
}

#[test]
fn a_click_selects_once_and_overflow_cancel_a_stray_up_or_an_up_outside_select_nothing() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(20),
  on_select = function(i) selected[#selected + 1] = i end}}
local function row(i) return lst.row_keys[i] end
local steps = {{
  -- a click on row 3: down, up
  function() dispatch({{ev("down", row(3), 150, 125)}}); dispatch({{ev("up", row(3), 150, 125)}}) end,
  -- the same in one packet
  function() dispatch({{ev("down", row(4), 150, 151), ev("up", row(4), 150, 151)}}) end,
  -- an up with no down of its own
  function() dispatch({{ev("up", row(2), 150, 99)}}) end,
  -- the down of one row and the up of another: the second row was never pressed
  function() dispatch({{ev("down", row(1), 150, 90)}}); dispatch({{ev("up", row(2), 150, 99)}}) end,
  -- cancel in between
  function() dispatch({{ev("down", row(5), 150, 177)}}); dispatch({{ev("cancel", row(5), 150, 177)}}); dispatch({{ev("up", row(5), 150, 177)}}) end,
  -- the up outside the row's published rectangle (inside = false)
  function() dispatch({{ev("down", row(5), 150, 177)}}); dispatch({{ev("up", row(5), 150, 230, false)}}) end,
  -- overflow in between: clicks nothing and releases the row
  function() dispatch({{ev("down", row(2), 150, 99)}}); dispatch({{}}, false, true); dispatch({{ev("up", row(2), 150, 99)}}) end,
  -- an overflow packet with a retained down and up
  function() dispatch({{ev("down", row(2), 150, 99), ev("up", row(2), 150, 99)}}, false, true) end,
  -- and a fresh click afterwards still selects
  function() dispatch({{ev("down", row(2), 150, 99)}}); dispatch({{ev("up", row(2), 150, 99)}}) end,
}}
local n = 0
window.on_opened(function()
  n = n + 1
  steps[n]()
  note("selected=" .. table.concat(selected, ","))
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    let expected = [
        "3", "3,4", "3,4", "3,4", "3,4", "3,4", "3,4", "3,4", "3,4,2",
    ];
    for (index, expected) in expected.iter().enumerate() {
        rig.opened();
        assert_eq!(note_of(&rig, "selected"), *expected, "step {}", index + 1);
    }
}

#[test]
fn a_hovered_row_and_a_pressed_row_show_the_white_highlight_in_the_themes_row_colours() {
    let tint =
        |red: u8, green: u8, blue: u8| ck_render_2d::Tint::from_straight(red, green, blue, 255);
    let script = |drive: &str| {
        format!(
            r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(20), on_select = function() end}}
local function row(i) return lst.row_keys[i] end
window.on_opened(function() {drive} end)
"#
        )
    };
    let shown = |rig: &Rig| -> Vec<(Rect, ck_render_2d::Tint)> {
        rig.sink
            .quads()
            .iter()
            .filter(|quad| (quad.width, quad.height) == (100.0, 26.0) && quad.x > -1.0e11)
            .map(|quad| ((quad.x, quad.y, quad.width, quad.height), quad.tint))
            .collect()
    };
    // The Places window is kind `window`: row_hover is its outline (#6e130f), row_pressed its hot colour (#a8261c). Row 3 is the
    // third row of the list at the content origin (106, 86): y 86 + 2 * 26 = 138.
    let mut rig = Rig::new(&script("dispatch({}, row(3))"));
    rig.opened();
    assert_eq!(
        shown(&rig),
        [((106.0, 138.0, 100.0, 26.0), tint(0x6e, 0x13, 0x0f))]
    );
    let mut rig = Rig::new(&script("dispatch({ev('down', row(3), 150, 125)}, row(3))"));
    rig.opened();
    assert_eq!(
        shown(&rig),
        [((106.0, 138.0, 100.0, 26.0), tint(0xa8, 0x26, 0x1c))]
    );
    // The highlight is the shared `white` texture: nothing loads the old button art.
    assert!(
        !std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/ui/button")
            .exists()
    );
}

#[test]
fn rows_have_the_hover_and_pressed_looks_drawn_by_quads_parked_when_unused() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(20), on_select = function() end}}
local function row(i) return lst.row_keys[i] end
local steps = {{
  function() dispatch({{}}, row(3)) end,
  function() dispatch({{ev("down", row(3), 150, 125)}}, row(3)) end,
  function() dispatch({{ev("up", row(3), 150, 125)}}, row(3)) end,
  function() dispatch({{}}, false) end,
}}
local n = 0
window.on_opened(function() n = n + 1; steps[n]() end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    let parked = |rect: Rect| rect.0 < -1.0e11;
    let before = highlights(&rig);
    assert_eq!(before.len(), 2, "a hover and a pressed quad");
    assert!(parked(before[0]) && parked(before[1]), "{before:?}");
    // Hover on row 3 (local y 52): the hover quad moves onto it.
    rig.opened();
    let hovered = highlights(&rig);
    assert_eq!((hovered[0].0, hovered[0].1), (106.0, 86.0 + 52.0));
    assert!(parked(hovered[1]));
    // Pressed: the pressed quad takes the row and the hover look steps aside.
    rig.opened();
    let pressed = highlights(&rig);
    assert!(parked(pressed[0]), "{pressed:?}");
    assert_eq!((pressed[1].0, pressed[1].1), (106.0, 86.0 + 52.0));
    // Released while still over it: hover again.
    rig.opened();
    let released = highlights(&rig);
    assert_eq!((released[0].0, released[0].1), (106.0, 86.0 + 52.0));
    assert!(parked(released[1]));
    // The pointer leaves.
    rig.opened();
    let left = highlights(&rig);
    assert!(parked(left[0]) && parked(left[1]), "{left:?}");
}

#[test]
fn a_press_selects_the_item_as_displayed_whatever_the_scroll_when_the_ui_dispatches_it() {
    let mut rig = Rig::new_fixture_body(&format!(
        "{}\nwindow.on_opened(function() lst:on_event({{kind = 'scroll', amount = -3, unit = 'lines'}}) end)",
        routed_script(20, true)
    ));
    let base = note_of(&rig, "base");
    // The player presses and releases on the row at (150, 125): item 2 (displayed at s = 0).
    assert_eq!(rig.key_at(150.0, 125.0), Some(format!("{base}_row_2")));
    rig.feed(&[
        PointerEvent::Down(points(150.0, 125.0)),
        PointerEvent::Up(points(150.0, 125.0)),
    ]);
    // Before the UI dispatches the click, the UI itself scrolls three rows: the item under that point is another now.
    rig.opened();
    assert_eq!(rig.key_at(150.0, 125.0), Some(format!("{base}_row_5")));
    rig.deliver_fixture_frame();
    // The click is on item 2: the one the player pointed at.
    assert!(
        notes(&rig).contains(&"select 2".to_owned()),
        "{:?}",
        notes(&rig)
    );
    assert!(!notes(&rig).contains(&"select 5".to_owned()));
}

#[test]
fn a_row_key_removed_while_its_row_is_pressed_cancels_the_press_and_selects_nothing() {
    let mut rig = Rig::new_fixture_body(&format!(
        "{}\nwindow.on_opened(function() lst:on_event({{kind = 'scroll', amount = -4, unit = 'lines'}}) end)",
        routed_script(20, true)
    ));
    let base = note_of(&rig, "base");
    rig.feed(&[PointerEvent::Down(points(150.0, 125.0))]);
    rig.deliver_fixture_frame();
    assert_eq!(rig.key_at(150.0, 125.0), Some(format!("{base}_row_2")));
    assert!(notes(&rig).contains(&format!("events=down:{base}_row_2:true")));
    // The UI scrolls four rows (104 points): row 2 leaves the view and its key is removed.
    let before = highlights(&rig);
    assert_eq!(before.len(), 2);
    rig.opened();
    assert_ne!(rig.key_at(150.0, 125.0), Some(format!("{base}_row_2")));
    // The router cancels the gesture it held for the removed key; the release then selects nothing.
    rig.feed(&[PointerEvent::Move(points(151.0, 126.0))]);
    rig.feed(&[PointerEvent::Up(points(151.0, 126.0))]);
    rig.deliver_fixture_frame();
    let lines = notes(&rig);
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&format!("cancel:{base}_row_2"))),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("select ")),
        "{lines:?}"
    );
    // And the pressed look went with the key: the pressed quad is parked (the hover quad follows the pointer, which is over another
    // row now).
    assert!(highlights(&rig)[1].0 < -1.0e11, "{:?}", highlights(&rig));
}

/// A list destroyed after an edit made outside a dispatch (which retires the removed rows' widgets until the next dispatch ends) is
/// not reached by a packet still naming a retired key: `how` is how it is destroyed, by itself or with its window.
fn retired_key_script(how: &str) -> String {
    format!(
        r#"{}
window.on_opened(function()
  local retired = lst.row_keys[2]
  -- Outside any dispatch (a game callback or a layout would do it): row 2 leaves the view; its widget is retired, not unregistered.
  lst:on_event({{kind = 'scroll', amount = -4, unit = 'lines'}})
  note("retired=" .. tostring(lst.row_keys[2] == nil and pointer.widget(retired) ~= nil))
  {how}
  note("closed=" .. tostring(lst.closed))
  note("widget=" .. tostring(pointer.widget(retired) ~= nil))
  -- The packet was routed against the frame the player pointed at: it still names the retired key, hovered and as events.
  local ok, err = pcall(dispatch, {{ev("down", retired, 150, 125), ev("up", retired, 150, 125)}}, retired)
  note("packet=" .. tostring(ok) .. " " .. tostring(ok and "" or err))
  ok, err = pcall(dispatch, {{}}, false)
  note("leave=" .. tostring(ok) .. " " .. tostring(ok and "" or err))
  ok, err = pcall(dispatch, {{}}, false, true)
  note("overflow=" .. tostring(ok) .. " " .. tostring(ok and "" or err))
end)
"#,
        routed_script(20, true)
    )
}

#[test]
fn a_retired_row_key_cannot_reach_a_list_destroyed_before_the_next_dispatch() {
    for how in ["lst:destroy()", "w:close()"] {
        let mut rig = Rig::new_fixture_body(&retired_key_script(how));
        rig.opened();
        let lines = notes(&rig);
        for expected in [
            "retired=true",
            "closed=true",
            "widget=false",
            "packet=true ",
            "leave=true ",
            "overflow=true ",
        ] {
            assert!(
                lines.contains(&expected.to_owned()),
                "{how}: {expected:?} missing: {lines:?}"
            );
        }
        assert!(
            !lines.iter().any(|line| line.starts_with("select ")),
            "{how}: {lines:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The attach seam
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_window_lays_an_attached_item_out_at_attach_and_in_the_callback_of_every_change() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "T", x = 100, y = 50, content_width = 200, content_height = 100}}
calls = {{}}
local function layout(_, ox, oy, clip)
  calls[#calls + 1] = string.format("%g,%g clip %g,%g,%g,%g", ox, oy, clip.x, clip.y, clip.width, clip.height)
end
stub = {{groups = {{}}, layout = layout, destroy = function() calls[#calls + 1] = "destroy" end}}
local function report(name)
  note(name .. "=" .. table.concat(calls, " | "))
  calls = {{}}
end
w:attach(stub)
report("attach")
local steps = {{
  function()
    dispatch({{ev("down", w.title_key, 110, 60)}})
    dispatch({{ev("move", w.title_key, 150, 90)}})
    dispatch({{ev("up", w.title_key, 150, 90)}})
    report("drag")
  end,
  function() w:move_to(300, 200); report("move") end,
  function() w:set_content_size(300, 150); report("size") end,
  function() windows.update_viewport(200, 150); report("clamp") end,
  function() w:close(); report("close") end,
}}
local n = 0
window.on_opened(function() n = n + 1; steps[n]() end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    // At attach (during start-up): the content origin (106, 86) and the content rectangle, both absolute.
    assert_eq!(note_of(&rig, "attach"), "106,86 clip 106,86,200,100");
    let step = |rig: &mut Rig, name: &str| {
        let published = rig.sink.count();
        rig.opened();
        (note_of(rig, name), rig.sink.count() - published)
    };
    // A drag: the move and the release each lay the item out at the window's new place (150 - 110 = 40 right, 90 - 60 = 30 down),
    // all in the callback that gave the events, and so in one published frame.
    assert_eq!(
        step(&mut rig, "drag"),
        (
            "146,116 clip 146,116,200,100 | 146,116 clip 146,116,200,100".to_owned(),
            1
        )
    );
    assert_eq!(
        step(&mut rig, "move"),
        ("306,236 clip 306,236,200,100".to_owned(), 1)
    );
    assert_eq!(
        step(&mut rig, "size"),
        ("306,236 clip 306,236,300,150".to_owned(), 1)
    );
    // The viewport shrinks to 200 by 150: the window is clamped back to (168, 120), which moves its content origin.
    assert_eq!(
        step(&mut rig, "clamp"),
        ("174,156 clip 174,156,300,150".to_owned(), 1)
    );
    // Closing the window destroys the item first.
    assert_eq!(step(&mut rig, "close").0, "destroy");
}

#[test]
fn a_list_is_drawn_above_the_content_which_is_above_the_chrome() {
    let script = format!(
        r#"{HEADER}
local label = require("lib.label")
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
label.new{{x = 0, y = 150, text = "M", group = w.content, owner = w}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(8)}}
other = windows.new{{title = "Other", x = 300, y = 300, content_width = 40, content_height = 40}}
"#
    );
    let rig = Rig::new_fixture_body(&script);
    let quads = rig.sink.quads();
    let content = (106.0, 86.0, 200.0, 200.0);
    let rows = (106.0, 86.0, 100.0, 130.0);
    let clip_of = |quad: &ck_render_2d::FrameQuad| {
        quad.clip
            .map(|clip| (clip.x, clip.y, clip.width, clip.height))
    };
    let first = |wanted: Option<Rect>| quads.iter().position(|quad| clip_of(quad) == wanted);
    let last = |wanted: Option<Rect>| quads.iter().rposition(|quad| clip_of(quad) == wanted);
    // The first window's chrome (no clip), then its content, then its list: chrome, content, rows. The second window was created
    // after, so its whole stack is above all of these.
    let chrome_first = first(None).expect("chrome quads");
    let content_first = first(Some(content)).expect("content quads");
    let rows_first = first(Some(rows)).expect("the list's quads");
    let content_last = last(Some(content)).expect("content quads");
    assert!(chrome_first < content_first, "chrome below content");
    assert!(content_last < rows_first, "content below the rows");
    // The other window's chrome has no clip and comes after the first window's rows.
    let other_chrome = quads
        .iter()
        .rposition(|quad| clip_of(quad).is_none())
        .expect("the other window");
    assert!(
        other_chrome > rows_first,
        "a window created later is above the earlier window's stack"
    );
}

#[test]
fn a_list_wholly_outside_the_content_area_hides_its_rows_and_shows_them_again_when_it_returns() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 100}}
-- 150 points below the content origin: wholly below the 100-point content area.
lst = lists.new{{window = w, x = 0, y = 150, width = 100, rows = 5, items = names(8), on_select = function() end}}
local steps = {{
  function() w:set_content_size(200, 400) end,
  function() w:set_content_size(200, 100) end,
}}
local n = 0
window.on_opened(function() n = n + 1; steps[n]() end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    let rows_visible = |rig: &Rig| {
        rig.sink.quads().iter().any(|quad| {
            quad.clip
                .is_some_and(|clip| (clip.y - 236.0).abs() < f64::EPSILON)
        })
    };
    assert!(
        !rows_visible(&rig),
        "outside the content area: the rows group is hidden"
    );
    // Its row keys publish nothing either.
    let regions = rig.harness.bridge.hit_regions().expect("regions");
    assert!(
        !regions
            .regions()
            .iter()
            .any(|region| region.key().contains("_row_"))
    );
    // The content grows to contain the list: the rows come back, with their keys.
    rig.opened();
    assert!(rows_visible(&rig), "inside again: shown");
    assert!(
        rig.harness
            .bridge
            .hit_regions()
            .expect("regions")
            .regions()
            .iter()
            .any(|region| region.key().contains("_row_"))
    );
    // And out again: hidden again.
    rig.opened();
    assert!(!rows_visible(&rig));
}

#[test]
fn closing_the_window_destroys_the_list_so_no_group_key_or_registration_remains() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(20), on_select = function() end}}
base = lst.base
rows_group = lst.rows_group
area = lst.area_key
row_key = lst.row_keys[2]
window.on_opened(function()
  w:close()
  note("closed=" .. tostring(lst.closed))
  -- The rows group is gone: using its token is an error.
  local ok, err = pcall(render.set_group_visible, rows_group, true)
  note("group=" .. tostring(ok) .. " " .. (tostring(err):match("no longer live") or tostring(err)))
  -- The registrations are gone: events for the old keys reach nobody.
  dispatch({{ev("scroll", area, 150, 100)}})
  dispatch({{{{kind = "scroll", key = row_key, x = 150, y = 120, amount = -3, unit = "lines", inside = true}}}})
  note("pos=" .. lst.pos)
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    let quads_before = rig.sink.quads().len();
    assert!(quads_before > 0);
    rig.opened();
    let lines = notes(&rig);
    assert!(lines.contains(&"closed=true".to_owned()), "{lines:?}");
    assert!(
        lines.contains(&"group=false no longer live".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"pos=0".to_owned()),
        "no event reached the closed list: {lines:?}"
    );
    assert!(
        rig.sink.quads().is_empty(),
        "no quad of the window or the list remains"
    );
    assert!(
        rig.harness
            .bridge
            .hit_regions()
            .expect("regions")
            .regions()
            .is_empty(),
        "nothing of the window or the list remains in the regions"
    );
}

#[test]
fn a_press_on_a_row_raises_its_window_through_the_owner_seam() {
    let script = format!(
        r#"{HEADER}
back = windows.new{{title = "Back", x = 20, y = 20, content_width = 200, content_height = 200}}
lst = lists.new{{window = back, x = 0, y = 0, width = 100, rows = 5, items = names(8), on_select = function() end}}
front = windows.new{{title = "Front", x = 300, y = 200, content_width = 60, content_height = 60}}
window.on_opened(function()
  note("before=" .. (windows.top() == front and "front" or "back"))
  dispatch({{ev("down", lst.row_keys[2], 60, 100)}})
  note("after=" .. (windows.top() == back and "back" or "front"))
  dispatch({{ev("up", lst.row_keys[2], 60, 100)}})
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    rig.opened();
    assert_eq!(note_of(&rig, "before"), "front");
    assert_eq!(note_of(&rig, "after"), "back");
}

// ---------------------------------------------------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn set_items_clamps_the_scroll_and_reuses_unchanged_rows_and_scroll_to_moves_the_least() {
    let script = format!(
        r#"{HEADER}
w = windows.new{{title = "Places", x = 100, y = 50, content_width = 200, content_height = 200}}
lst = lists.new{{window = w, x = 0, y = 0, width = 100, rows = 5, items = names(20), on_select = function() end}}
local function state(name)
  local n = 0
  for _ in pairs(lst.row_keys) do n = n + 1 end
  note(string.format("%s=pos=%g s=%g max=%g keys=%d", name, lst.pos, lst.s, lst.max_s, n))
end
local steps = {{
  function() lst:set_pos(lst.max_s); state("end") end,
  -- fewer items: the scroll is clamped to the new end (3 rows past the top)
  function() lst:set_items(names(8)); state("short") end,
  -- the same items again: nothing to draw
  function() lst:set_items(names(8)); state("same") end,
  -- one text changed
  function() local t = names(8); t[5] = "Fifth"; lst:set_items(t); state("changed") end,
  function() lst:set_items(names(20)); lst:set_pos(0); state("top") end,
  function() lst:scroll_to(10); state("to10") end,
  function() lst:scroll_to(1); state("to1") end,
  function() lst:scroll_to(7); state("to7") end,
  function() lst:scroll_to(7); state("to7again") end,
  function()
    note("bad0=" .. tostring(pcall(lst.scroll_to, lst, 0)))
    note("bad21=" .. tostring(pcall(lst.scroll_to, lst, 21)))
    note("badset=" .. tostring(pcall(lst.set_items, lst, {{1}})))
    lst:set_items({{}})
    state("empty")
  end,
}}
local n = 0
window.on_opened(function() n = n + 1; steps[n]() end)
"#
    );
    let mut rig = Rig::new_fixture_body(&script);
    let step = |rig: &mut Rig, name: &str| {
        let published = rig.sink.count();
        rig.opened();
        (note_of(rig, name), rig.sink.count() - published)
    };
    assert_eq!(step(&mut rig, "end").0, "pos=390 s=390 max=390 keys=5");
    // The 20 rows shrink to 8: max_s = 3 * 26 = 78 and the scroll is clamped to it; rows 4 to 8 are in view.
    assert_eq!(
        step(&mut rig, "short"),
        ("pos=78 s=78 max=78 keys=5".to_owned(), 1)
    );
    assert_eq!(
        step(&mut rig, "same"),
        ("pos=78 s=78 max=78 keys=5".to_owned(), 0),
        "no scene call for unchanged text"
    );
    assert_eq!(
        step(&mut rig, "changed"),
        ("pos=78 s=78 max=78 keys=5".to_owned(), 1)
    );
    assert_eq!(step(&mut rig, "top").0, "pos=0 s=0 max=390 keys=5");
    // scroll_to: row 10 is below the view: the least move puts its bottom edge at the view's (pos = 10 * 26 - 130).
    assert_eq!(step(&mut rig, "to10").0, "pos=130 s=130 max=390 keys=5");
    // Row 1 is above: its top edge at the view's.
    assert_eq!(step(&mut rig, "to1").0, "pos=0 s=0 max=390 keys=5");
    // Row 7 from the top: its bottom (182) is 52 below the view's: two whole rows, so rows 3 to 7 are in view.
    assert_eq!(step(&mut rig, "to7").0, "pos=52 s=52 max=390 keys=5");
    // Already wholly visible: no change and no scene call.
    assert_eq!(
        step(&mut rig, "to7again"),
        ("pos=52 s=52 max=390 keys=5".to_owned(), 0)
    );
    rig.opened();
    assert_eq!(note_of(&rig, "bad0"), "false");
    assert_eq!(note_of(&rig, "bad21"), "false");
    assert_eq!(note_of(&rig, "badset"), "false");
    assert_eq!(note_of(&rig, "empty"), "pos=0 s=0 max=0 keys=0");
}

// The ledger lifecycle and rows are covered by `location_tests`; this module keeps the windowed places-list witness.

// ---------------------------------------------------------------------------------------------------------------------
// The places-list window witness, offline
// ---------------------------------------------------------------------------------------------------------------------

/// The log a successful run leaves (the driver's sidecar after the app's lines), with a scroll of `scroll` points that put the
/// place at the click on `index`-th name.
fn captured(scroll: u32, title: &str, expected: &str) -> String {
    let list_row = (45 + scroll) / 26 + 1;
    format!(
        "INFO  lua.ui: places window opened\nINFO  lua.ui: places list opened 95 items 12 rows\nINFO  lua.ui: places list geometry y=0 row_height=26 click_y=45\nINFO  lua.ui: places list s={scroll}\nINFO  lua.ui: places list click row {list_row} visible 2\nINFO  lua.ui: place window expected {expected}\nINFO  lua.ui: place window opened {title}\nINFO  lua.ui: location tab people: 1280\nINFO  lua.ui: place window closed\nINFO  lua.ui: places title clicked\nINFO  lua.ui: places window closed\nINFO  lua.ui: close requested by key\nINFO  window: window closed\ncamera 0 1 1\nbefore-list-scroll\nafter-list-scroll\n"
    )
}

#[test]
fn the_judgment_accepts_a_good_run_and_refuses_each_way_a_run_can_be_wrong() {
    assert_eq!(
        places_list::validate(&captured(16, "Cluain Mhic Nóis", "Cluain Mhic Nóis")),
        Ok(())
    );
    assert_eq!(
        places_list::validate(&captured(260, "Daimhliag", "Daimhliag")),
        Ok(())
    );
    for (wrong, why) in [
        (captured(0, "A", "A"), "scrolled by 0"),
        (
            captured(15, "A", "A"),
            "positive even whole number of UI pixels",
        ),
        (captured(16, "A", "B"), "the click opened another place"),
        (
            captured(16, "A", "A").replace(
                "before-list-scroll\nafter-list-scroll\n",
                "after-list-scroll\nbefore-list-scroll\n",
            ),
            "markers out of order",
        ),
        (
            captured(16, "A", "A")
                .replace("before-list-scroll\n", "before-list-scroll\ncamera 0 2 2\n"),
            "wheel over the list changed the map camera",
        ),
        (
            captured(16, "A", "A").replace("INFO  lua.ui: places window closed\n", ""),
            "is missing or out of order",
        ),
        (
            captured(16, "A", "A")
                .replace("INFO  lua.ui: places list opened 95 items 12 rows\n", ""),
            "places list opened",
        ),
        (
            captured(16, "A", "A").replace("INFO  lua.ui: location tab people: 1280\n", ""),
            "Location tab's people line has no count",
        ),
        (
            captured(16, "A", "A").replace("people: 1280", "people: many"),
            "the people line has no count",
        ),
    ] {
        assert!(places_list::validate(&wrong).is_err(), "{why}");
    }
}

/// The fixture's geometry, through the real code path: production `main.lua` over 94 places publishes its docked ledger, and the fixture
/// follows the resulting row and close-button coordinates through the real pointer router.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the window witness exercises the published marker protocol as one ordered interaction"
)]
fn the_witness_fixture_reaches_each_marker_at_the_drivers_coordinates() {
    let markers = crate::testing::Dir::new();
    let script = format!(
        r#"MARKER_DIR = [==[{markers}]==]
callbacks = {{}}
places_data = {{}}
for i = 1, 94 do
  places_data[i] = {{id = "p" .. i, name = "Place " .. i, kind = "town", size = 2}}
end
game = {{
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  places = function() return places_data end,
}}
{witness}
callbacks.time({{day = 1, month = 1, year = 768, hour = 0, paused = false, speed = 1}})
local top = require("lib.window")
window.on_opened(function()
  local current = top.top()
  window.record("top=" .. (current and current.title or "none"))
end)
"#,
        markers = markers.0.display(),
        witness = places_list::script(),
    );
    let mut rig = Rig::new_fixture_body(&script);
    let marker = |name: &str| markers.0.join(name).exists();
    // The markers wait one frame after the callback that made them.
    assert!(!marker("places-button-ready"));
    rig.deliver_fixture_frame_at(400, 320, 2);
    assert!(!marker("places-button-ready"));
    rig.deliver_fixture_frame();
    assert!(marker("places-button-ready"));
    // The icon is first created against the requested 800x600 view, then the first frame re-anchors it to 400x320.
    // The native driver must not use that creation-time point.
    let created = std::fs::read_to_string(markers.0.join("places-button-created-center"))
        .expect("the locations icon's creation center");
    let mut created = created
        .split_whitespace()
        .map(|value| value.parse::<f64>().unwrap());
    let (created_x, created_y) = (created.next().unwrap(), created.next().unwrap());
    assert!(
        rig.key_at(created_x, created_y).is_none(),
        "creation-time point ({created_x}, {created_y}) unexpectedly hits the resized view"
    );
    // The final center is captured after the app's first-frame relayout and scene publication.
    let center = std::fs::read_to_string(markers.0.join("places-button-center"))
        .expect("the locations icon center marker");
    let mut center = center
        .split_whitespace()
        .map(|value| value.parse::<f64>().unwrap());
    let (x, y) = (center.next().unwrap(), center.next().unwrap());
    let bar_key = rig.key_at(x, y).unwrap_or_else(|| {
        panic!(
            "the locations button center ({x}, {y}) misses all published regions: {:?}",
            rig.harness
                .bridge
                .hit_regions()
                .unwrap()
                .regions()
                .iter()
                .map(ck_render_2d::HitRegion::key)
                .collect::<Vec<_>>()
        )
    });
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame();
    assert!(!marker("places-ready"), "queued for the next frame");
    rig.deliver_fixture_frame();
    assert!(marker("places-ready"));
    assert!(
        !rig.harness
            .bridge
            .hit_regions()
            .unwrap()
            .regions()
            .iter()
            .any(|region| region.key() == bar_key),
        "the locations icon is hidden while the ledger is open"
    );
    let geometry = std::fs::read_to_string(markers.0.join("places-window-geometry"))
        .expect("the dropdown geometry marker");
    let geometry: Vec<f64> = geometry
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    let row_x = geometry[0] + 26.0;
    let row_y = geometry[1] + geometry[4] + 45.0;
    let key = rig.key_at(row_x, row_y).unwrap_or_else(|| {
        panic!("no row at derived point ({row_x}, {row_y}) from geometry {geometry:?}")
    });
    assert!(key.contains("_row_2"), "{key}");
    assert!(
        rig.key_at(row_x, row_y - 45.0)
            .is_none_or(|key| !key.contains("_row_"))
    );
    // The driver's wheel (the Cocoa delta, 15 points toward later items) scrolls the list by 16 points.
    rig.feed(&[scroll_points(row_x, row_y, -15)]);
    rig.deliver_fixture_frame();
    rig.deliver_fixture_frame();
    assert!(marker("list-scrolled"));
    // The driver's click on the same point is now on the second row: it opens that place's window.
    rig.feed(&[
        PointerEvent::Down(points(row_x, row_y)),
        PointerEvent::Up(points(row_x, row_y)),
    ]);
    rig.deliver_fixture_frame();
    rig.deliver_fixture_frame();
    assert!(marker("place-ready"));
    rig.opened();
    assert_eq!(note_of(&rig, "top"), "Place 2");
    let title = std::fs::read_to_string(markers.0.join("places-title-center"))
        .expect("the ledger close-button center marker");
    let mut title = title.split_whitespace();
    let close_key = title.next().unwrap();
    let x: f64 = title.next().unwrap().parse().unwrap();
    let y: f64 = title.next().unwrap().parse().unwrap();
    assert_eq!(rig.key_at(x, y).as_deref(), Some(close_key));
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame();
    rig.deliver_fixture_frame();
    assert!(marker("places-title-clicked"));
    assert!(marker("places-closed"));
}

/// A pixel-open UI publishes marker coordinates in UI units, while the native window driver injects window points. The witness carries
/// the frame's UI extent so the driver can scale each target by the live content-view extent instead of assuming a backing scale.
fn places_witness_fixture_script(markers: &crate::testing::Dir) -> String {
    format!(
        r#"MARKER_DIR = [==[{markers}]==]
callbacks = {{}}
places_data = {{}}
for i = 1, 94 do
  places_data[i] = {{id = "p" .. i, name = "Place " .. i, kind = "town", size = 2}}
end
game = {{
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  places = function() return places_data end,
}}
{witness}
callbacks.time({{day = 1, month = 1, year = 768, hour = 0, paused = false, speed = 1}})
"#,
        markers = markers.0.display(),
        witness = places_list::script(),
    )
}

/// The time-bar driver's coordinates against the shipped pixel-open app, with the real UI VM's current extent.
fn time_bar_witness_fixture_script() -> String {
    format!(
        r#"callbacks = {{}}
places_data = {{}}
for i = 1, 94 do
  places_data[i] = {{id = "p" .. i, name = "Place " .. i, kind = "town", size = 2}}
end
game = {{
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  places = function() return places_data end,
}}
{}
callbacks.time({{day = 1, month = 1, year = 768, hour = 0, paused = false, speed = 3}})
"#,
        time_bar::script()
    )
}

#[test]
fn pixel_open_time_bar_driver_units_convert_to_live_window_points() {
    let script = time_bar_witness_fixture_script();
    let mut rig = Rig::new_fixture_body_in_pixels(&script);
    rig.deliver_fixture_frame_at(400, 320, 2);
    rig.deliver_fixture_frame();

    let extent = notes(&rig)
        .into_iter()
        .find_map(|line| line.strip_prefix("time-bar UI extent ").map(str::to_owned))
        .expect("the real frame callback publishes its live UI extent");
    let dimensions: Vec<f64> = extent
        .split_whitespace()
        .map(|value| value.parse().expect("numeric UI extent"))
        .collect();
    assert_eq!(dimensions, [800.0, 640.0]);

    // The native fixture coordinates name UI units: the fast-forward button, speed box 5, and speed box 3 drag start.
    let fast_forward = rig.key_at(83.0, 27.0).expect("fast-forward hit region");
    assert!(fast_forward.starts_with("button_"), "{fast_forward}");
    assert_eq!(rig.key_at(174.0, 27.0).as_deref(), Some("speed_5"));
    assert_eq!(rig.key_at(142.0, 27.0).as_deref(), Some("speed_3"));
    assert_eq!(rig.key_at(147.0, 27.0).as_deref(), Some("speed_3"));

    // The 400x320-point content view is 800x640 UI units at this scale. The same conversion as the native driver maps the target
    // to window points; mapping those points back through the live extents lands on the same published hit region.
    let point = (83.0 * 400.0 / dimensions[0], 27.0 * 320.0 / dimensions[1]);
    assert_eq!(point, (41.5, 13.5));
    assert_eq!(
        rig.key_at(
            point.0 * dimensions[0] / 400.0,
            point.1 * dimensions[1] / 320.0,
        )
        .as_deref(),
        Some(fast_forward.as_str())
    );
    // Feeding the same numeric pair as points (the old bug) addresses UI y=54, below the time bar, and misses the control.
    assert_ne!(
        rig.key_at(83.0 * dimensions[0] / 400.0, 27.0 * dimensions[1] / 320.0)
            .as_deref(),
        Some(fast_forward.as_str())
    );
}

#[test]
fn the_witness_pixel_coordinates_convert_to_window_points_for_each_places_target() {
    let markers = crate::testing::Dir::new();
    let script = places_witness_fixture_script(&markers);
    let mut rig = Rig::new_fixture_body_in_pixels(&script);
    let marker = |name: &str| markers.0.join(name).exists();
    rig.deliver_fixture_frame_at(400, 320, 2);
    rig.deliver_fixture_frame();

    // The harness attaches a 400x320 point window to an 800x640 drawable. At scale 1, the UI sees 800x640 units.
    let to_window_points = |x: f64, y: f64, ui_width: f64, ui_height: f64| {
        (x * 400.0 / ui_width, y * 320.0 / ui_height)
    };
    let click = |rig: &mut Rig, x: f64, y: f64, ui_width: f64, ui_height: f64| {
        let (x, y) = to_window_points(x, y, ui_width, ui_height);
        rig.feed(&[
            PointerEvent::Down(points(x, y)),
            PointerEvent::Up(points(x, y)),
        ]);
    };

    let center = std::fs::read_to_string(markers.0.join("places-button-center"))
        .expect("the published locations icon center");
    let center: Vec<f64> = center
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(&center[2..], &[800.0, 640.0]);
    let (button_x, button_y) = to_window_points(center[0], center[1], center[2], center[3]);
    let button_key = rig
        .key_at(center[0], center[1])
        .expect("the published icon hit");
    assert_eq!(
        rig.key_at(button_x * center[2] / 400.0, button_y * center[3] / 320.0)
            .as_deref(),
        Some(button_key.as_str()),
        "the converted window point maps back to the published UI hit"
    );
    click(&mut rig, center[0], center[1], center[2], center[3]);
    rig.deliver_fixture_frame();
    rig.deliver_fixture_frame();
    assert!(marker("places-ready"));

    let geometry = std::fs::read_to_string(markers.0.join("places-window-geometry"))
        .expect("the ledger geometry and its UI extent");
    let geometry: Vec<f64> = geometry
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    let row_x = geometry[0] + 26.0;
    let row_y = geometry[1] + geometry[4] + 45.0;
    let (scroll_x, scroll_y) = to_window_points(row_x, row_y, geometry[5], geometry[6]);
    rig.feed(&[scroll_points(scroll_x, scroll_y, -15)]);
    rig.deliver_fixture_frame();
    rig.deliver_fixture_frame();
    assert!(marker("list-scrolled"));
    click(&mut rig, row_x, row_y, geometry[5], geometry[6]);
    rig.deliver_fixture_frame();
    rig.deliver_fixture_frame();
    assert!(marker("place-ready"));
    let records = notes(&rig);
    let clicked = records
        .iter()
        .position(|line| line.starts_with("places list click row "))
        .expect("the click callback records its row");
    let expected = records
        .iter()
        .position(|line| line.starts_with("place window expected "))
        .expect("the click callback records its expected place");
    let opened = records
        .iter()
        .position(|line| line.starts_with("place window opened "))
        .expect("the witness records the opened place panel");
    assert!(clicked < expected && expected < opened, "{records:?}");
    assert_eq!(
        records[expected].strip_prefix("place window expected "),
        records[opened].strip_prefix("place window opened ")
    );

    assert!(rig.escape());
    rig.deliver_fixture_frame();
    assert!(marker("place-closed"));
    let title = std::fs::read_to_string(markers.0.join("places-title-center"))
        .expect("the title close-button center and its UI extent");
    let mut title = title.split_whitespace();
    let _close_key = title.next().unwrap();
    let x: f64 = title.next().unwrap().parse().unwrap();
    let y: f64 = title.next().unwrap().parse().unwrap();
    let ui_width: f64 = title.next().unwrap().parse().unwrap();
    let ui_height: f64 = title.next().unwrap().parse().unwrap();
    click(&mut rig, x, y, ui_width, ui_height);
    rig.deliver_fixture_frame();
    assert!(marker("places-closed"));
}

#[test]
fn the_open_ledger_docks_and_reanchors_at_both_scales() {
    let main_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let main = std::fs::read_to_string(main_path).expect("the production UI script");
    {
        let scale = 2_i64;
        let script = format!(
            r#"
callbacks = {{}}
places_data = {{{{id="one", name="One", kind="town", size=1}}}}
game = {{
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  places = function() return places_data end,
}}
function note(text) window.record(text) end
local button_library = require("lib.button")
local real_button_new = button_library.new
local location_button
button_library.new = function(spec)
  local created = real_button_new(spec)
  if spec.icon == "location" then location_button = created end
  return created
end
local windows = require("lib.window")
local real_new = windows.new
windows.new = function(spec)
  local created = real_new(spec)
  if spec.title == "ledger" then
    local real_move_to = created.move_to
    function created:move_to(x, y)
      real_move_to(self, x, y)
      local left, top = self:position()
      local width, height = self:size()
      note(string.format("places-geometry %g %g %g %g", left, top, width, height))
    end
  end
  return created
end
local real_on_update = frame.on_update
frame.on_update = function(callback)
  real_on_update(function(update)
    callback(update)
    -- The production layout first uses window.open's resolved size, then this frame's viewport. Observe after its relayout.
    if location_button ~= nil then
      local width, height = location_button:size()
      note(string.format("location-button %s %g %g", location_button.key,
        location_button.x + width / 2, location_button.y + height / 2))
    end
  end)
end
{main}
callbacks.time({{day=1, month=1, year=768, hour=0, paused=false, speed=1}})
"#
        );
        let mut rig = Rig::new_fixture_body(&script);
        rig.deliver_fixture_frame_at(800, 600, u32::try_from(scale).unwrap());
        let line = notes(&rig)
            .into_iter()
            .find(|line| line.starts_with("location-button "))
            .expect("the locations icon coordinates");
        let mut fields = line.split_whitespace();
        let _ = fields.next();
        let key = fields.next().unwrap();
        let x: f64 = fields.next().unwrap().parse().unwrap();
        let y: f64 = fields.next().unwrap().parse().unwrap();
        assert_eq!(rig.key_at(x, y).as_deref(), Some(key));
        rig.feed(&[
            PointerEvent::Down(points(x, y)),
            PointerEvent::Up(points(x, y)),
        ]);
        rig.deliver_fixture_frame_at(800, 600, u32::try_from(scale).unwrap());
        let geometry = |line: &str| -> Vec<i64> {
            line.split_whitespace()
                .skip(1)
                .map(|part| part.parse().unwrap())
                .collect()
        };
        let records = notes(&rig);
        let opened = records
            .iter()
            .find(|line| line.starts_with("places-geometry "))
            .expect("the ledger is positioned after opening");
        let opened = geometry(opened);
        let margin = 8;
        let snap = |value: i64| ((value + scale / 2) / scale) * scale;
        assert_eq!((opened[0] + opened[2], opened[1]), (snap(800), margin));

        rig.deliver_fixture_frame_at(1200, 700, u32::try_from(scale).unwrap());
        let moved = notes(&rig)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("places-geometry "))
            .expect("the ledger is repositioned after resize");
        let moved = geometry(&moved);
        assert_eq!((moved[0] + moved[2], moved[1]), (snap(1200), margin));
    }
}

/// The witness's configuration is one the application accepts, for either backend (the window targets write it to their scratch
/// directory and start the shipped binary on it).
#[test]
fn the_witness_configuration_loads_for_each_backend() {
    for backend in ["native-metal", "glfw-vulkan"] {
        let text = places_list::config_text(backend, std::path::Path::new("/tmp/codekarl-saves"));
        let _ = crate::config::load_text_for_test(&text);
    }
}
