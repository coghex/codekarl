//! The widget library (`scripts/ui/lib/`) run for real in a UI VM over a scene that records its calls: the anchors, units and
//! snapping, the label and the button, the pointer dispatcher, and the library-based `main.lua` (layout against the window's
//! current size and the idle rule).
//!
//! Nothing sleeps: each script runs to the end of its main chunk, or a `frame_update` is posted and awaited.

use std::collections::BTreeMap;

use ck_lua::ConfigValue;

use crate::readout_tests::Readout;

/// The records of a script (`window.record`), without the `record ` prefix and without the marks that split the scene calls.
fn records(readout: &Readout) -> Vec<String> {
    readout
        .entries()
        .iter()
        .filter_map(|entry| entry.strip_prefix("record ").map(str::to_owned))
        .filter(|record| record != "mark")
        .collect()
}

/// A quad's rectangle as the scene calls leave it.
type Rect = (i64, i64, i64, i64);

/// Replays `create`, `set_position`, `set_rect` and `destroy` entries into the quads that are alive and where they are.
fn quads(entries: &[String]) -> BTreeMap<String, Rect> {
    let mut alive = BTreeMap::new();
    for entry in entries {
        let parts: Vec<&str> = entry.split(' ').collect();
        let numbers = |from: usize| -> Vec<i64> {
            parts[from..]
                .iter()
                .map(|part| part.parse().expect("a number"))
                .collect()
        };
        match parts[0] {
            "create" => {
                let n = numbers(3);
                alive.insert(parts[1].to_owned(), (n[0], n[1], n[2], n[3]));
            }
            "set_position" => {
                let n = numbers(2);
                let rect = alive.get_mut(parts[1]).expect("a live quad");
                (rect.0, rect.1) = (n[0], n[1]);
            }
            "set_rect" => {
                let n = numbers(2);
                alive.insert(parts[1].to_owned(), (n[0], n[1], n[2], n[3]));
            }
            "destroy" => {
                alive.remove(parts[1]);
            }
            _ => {}
        }
    }
    alive
}

/// The quads of `names` that are not parked.
fn in_place(alive: &BTreeMap<String, Rect>, names: &[&str], hidden: i64) -> Vec<String> {
    names
        .iter()
        .filter(|name| alive.get(**name).is_some_and(|rect| rect.0 != hidden))
        .map(|name| (*name).to_owned())
        .collect()
}

const GO_BUTTON: &str = r#"
local button = require("lib.button")
local pointer = require("lib.pointer")
local clicks = 0
local b = button.new{x = 10, y = 20, text = "Go", on_click = function() clicks = clicks + 1 end}
local key = b.key
local function packet(over, events, overflow, held)
  return {x = 0, y = 0, held = held or false, over = over, overflow = overflow or false, events = events}
end
local function ev(kind, x, y)
  return {kind = kind, x = x, y = y, key = key}
end
-- Dispatches a packet and checks what `dispatch` reported.
local function send(p, expect)
  local changed = pointer.dispatch(p)
  assert(changed == expect, "dispatch reported " .. tostring(changed) .. ", expected " .. tostring(expect))
end
local function mark()
  window.record("mark")
end
local function clicked()
  window.record("clicks " .. clicks)
end
"#;

// ---------------------------------------------------------------------------------------------------------------------
// `ui`: anchors, units and snapping, keys
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_nine_anchors_at_two_window_sizes_are_literals() {
    let readout = Readout::script(
        r#"
local ui = require("lib.ui")
for _, size in ipairs{{800, 600}, {1280, 720}} do
  for _, name in ipairs{"top_left", "top", "top_right", "left", "centre", "right", "bottom_left", "bottom", "bottom_right"} do
    local x, y = ui.anchor(name, 100, 40, 8, 8, size[1], size[2])
    window.record(size[1] .. "x" .. size[2] .. " " .. name .. " " .. x .. " " .. y)
  end
end
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        [
            "800x600 top_left 8 8",
            "800x600 top 358 8",
            "800x600 top_right 708 8",
            "800x600 left 8 288",
            "800x600 centre 358 288",
            "800x600 right 708 288",
            "800x600 bottom_left 8 568",
            "800x600 bottom 358 568",
            "800x600 bottom_right 708 568",
            "1280x720 top_left 8 8",
            "1280x720 top 598 8",
            "1280x720 top_right 1188 8",
            "1280x720 left 8 348",
            "1280x720 centre 598 348",
            "1280x720 right 1188 348",
            "1280x720 bottom_left 8 688",
            "1280x720 bottom 598 688",
            "1280x720 bottom_right 1188 688",
        ]
    );
}

#[test]
fn an_unknown_anchor_is_an_error() {
    let readout = Readout::script(
        r#"
local ui = require("lib.ui")
local ok, err = pcall(ui.anchor, "middle", 1, 1, 0, 0, 10, 10)
window.record(tostring(ok) .. " " .. tostring(err))
"#,
        None,
    );
    let record = &records(&readout)[0];
    assert!(
        record.starts_with("false ") && record.contains("`middle` is not an anchor"),
        "{record}"
    );
}

#[test]
fn snapping_is_to_the_nearest_ui_pixel_halves_up_and_a_size_is_at_least_one() {
    let readout = Readout::script(
        r#"
local ui = require("lib.ui")
window.record("P " .. ui.P)
for _, v in ipairs{0, 0.9, 1, 1.5, 2.49, 2.5, 3, -1, -1.5, 389.5} do
  window.record(v .. " -> " .. ui.snap(v) .. " size " .. ui.snap_size(v))
end
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        [
            "P 2",
            "0 -> 0 size 2",
            "0.9 -> 0 size 2",
            "1 -> 2 size 2",
            "1.5 -> 2 size 2",
            "2.49 -> 2 size 2",
            "2.5 -> 2 size 2",
            "3 -> 4 size 4",
            "-1 -> 0 size 2",
            "-1.5 -> -2 size 2",
            "389.5 -> 390 size 390",
        ]
    );
}

#[test]
fn at_three_points_per_pixel_the_examples_of_the_spec_hold() {
    let readout = Readout::script(
        r#"
local ui = require("lib.ui")
local panel = require("lib.panel")
window.record("P " .. ui.P)
-- x = 8 snaps to 9 at P = 3.
panel.new{x = 8, y = 8, width = 30, height = 30}
-- A centred panel 7 UI pixels wide (21 points) in an 800-point window: (800 - 21) / 2 = 389.5, which snaps to 390.
local x, y = ui.anchor("centre", 21, 21, 0, 0, 800, 600)
window.record("centre " .. x .. " " .. y)
panel.new{x = x, y = y, width = 21, height = 21}
"#,
        Some(3),
    );
    assert_eq!(records(&readout), ["P 3", "centre 390 291"]);
    assert_eq!(
        readout.entries(),
        [
            "record P 3",
            "load ui/font/panel.png",
            "create q1 ui/font/panel.png 9 9 30 30",
            "record centre 390 291",
            "create q2 ui/font/panel.png 390 291 21 21",
        ]
    );
}

#[test]
fn keys_are_unique_and_within_the_hosts_pattern() {
    let readout = Readout::script(
        r#"
local ui = require("lib.ui")
window.record(ui.key("button") .. " " .. ui.key("button") .. " " .. ui.key("a1_b"))
for _, prefix in ipairs{"", "Bad", "has space", "x-y", string.rep("a", 48)} do
  local ok, err = pcall(ui.key, prefix)
  window.record(tostring(ok))
end
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        [
            "button_1 button_2 a1_b_3",
            "false",
            "false",
            "false",
            "false",
            "false"
        ]
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// label, panel, button: the quads
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_label_and_a_panel_snap_what_they_are_given_and_measure_in_points() {
    let readout = Readout::script(
        r#"
local label = require("lib.label")
local panel = require("lib.panel")
local p = panel.new{x = 5, y = 7, width = 31, height = 0}
window.record("panel " .. p.x .. " " .. p.y .. " " .. p.width .. " " .. p.height)
local l = label.new{x = 8, y = 8, text = "Go"}
window.record("size " .. table.concat({l:size()}, " "))
l:set_text("G")
window.record("size " .. table.concat({l:size()}, " "))
l:move(9, 9)
l:destroy()
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        ["panel 6 8 32 2", "size 24 22", "size 12 22"]
    );
    assert_eq!(
        readout.entries()[..9],
        [
            "load ui/font/panel.png",
            "create q1 ui/font/panel.png 6 8 32 2",
            "record panel 6 8 32 2",
            "load ui/font/glyphs/U+0047.png",
            "create q2 ui/font/glyphs/U+0047.png 8 8 10 22",
            "load ui/font/glyphs/U+006F.png",
            "create q3 ui/font/glyphs/U+006F.png 20 8 10 22",
            "record size 24 22",
            "destroy q3",
        ]
    );
    // `move` to 9, 9 snaps to 10, 10 and repositions the one glyph; `destroy` removes it.
    assert_eq!(
        readout.entries()[9..],
        ["record size 12 22", "set_position q2 10 10", "destroy q2"]
    );
}

#[test]
fn a_buttons_quads_are_literal_rectangles_at_two_points_per_pixel() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}window.record('size ' .. table.concat({{b:size()}}, ' '))"),
        None,
    );
    assert_eq!(
        readout.entries(),
        [
            "load ui/button/normal.png",
            "create q1 ui/button/normal.png 10 20 32 30",
            "load ui/button/hover.png",
            "create q2 ui/button/hover.png -1000000 -1000000 32 30",
            "load ui/button/pressed.png",
            "create q3 ui/button/pressed.png -1000000 -1000000 32 30",
            "load ui/font/glyphs/U+0047.png",
            "create q4 ui/font/glyphs/U+0047.png 14 24 10 22",
            "load ui/font/glyphs/U+006F.png",
            "create q5 ui/font/glyphs/U+006F.png 26 24 10 22",
            "set_hit button_1 10 20 32 30",
            "record size 32 30",
        ]
    );
}

#[test]
fn a_buttons_quads_are_literal_rectangles_at_three_points_per_pixel() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}window.record('size ' .. table.concat({{b:size()}}, ' '))"),
        Some(3),
    );
    assert_eq!(
        readout.entries(),
        [
            "load ui/button/normal.png",
            "create q1 ui/button/normal.png 9 21 48 45",
            "load ui/button/hover.png",
            "create q2 ui/button/hover.png -999999 -999999 48 45",
            "load ui/button/pressed.png",
            "create q3 ui/button/pressed.png -999999 -999999 48 45",
            "load ui/font/glyphs/U+0047.png",
            "create q4 ui/font/glyphs/U+0047.png 15 27 15 33",
            "load ui/font/glyphs/U+006F.png",
            "create q5 ui/font/glyphs/U+006F.png 33 27 15 33",
            "set_hit button_1 9 21 48 45",
            "record size 48 45",
        ]
    );
}

#[test]
fn every_quad_and_hit_edge_is_a_multiple_of_the_pixel_scale() {
    for scale in [2, 3, 5] {
        let readout = Readout::script(
            r#"
local button = require("lib.button")
local label = require("lib.label")
local panel = require("lib.panel")
local ui = require("lib.ui")
local b = button.new{x = 13, y = 17, text = "Hi", on_click = function() end}
b:move(31, 29)
b:set_text("Hello")
panel.new{x = 7, y = 9, width = 55, height = 23}:set_rect(11, 13, 17, 19)
local l = label.new{x = 1, y = 2, text = "ab"}
l:move(23, 41)
local x, y = ui.anchor("bottom_right", 33, 22, -8, -8, 801, 601)
panel.new{x = x, y = y, width = 33, height = 22}
"#,
            Some(scale),
        );
        let mut seen = 0;
        for entry in readout.entries() {
            let parts: Vec<&str> = entry.split(' ').collect();
            let from = match parts[0] {
                "create" => 3,
                "set_position" | "set_rect" | "set_hit" => 2,
                _ => continue,
            };
            for part in &parts[from..] {
                let value: i64 = part.parse().expect("a number");
                assert_eq!(value % scale, 0, "P={scale}: {entry}");
                seen += 1;
            }
        }
        assert!(seen > 40, "P={scale}: checked {seen} edges");
    }
}

#[test]
fn a_buttons_background_is_one_quad_in_place_and_its_glyphs_stay_above() {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}mark()\nsend(packet(key, {{}}), true)\nmark()\nsend(packet(key, {{ev('down', 12, 22)}}, false, true), true)\nmark()\nsend(packet(false, {{ev('up', 500, 500)}}), true)\nmark()\nb:set_text('Hello')\nmark()"
        ),
        None,
    );
    let entries = readout.entries();
    let first = entries
        .iter()
        .position(|e| e == "record mark")
        .expect("a mark");
    // Painter order is creation order: the three backgrounds precede every glyph.
    let creates: Vec<&String> = entries[..first]
        .iter()
        .filter(|e| e.starts_with("create"))
        .collect();
    assert!(
        creates[..3].iter().all(|e| e.contains("ui/button/")),
        "{creates:?}"
    );
    assert!(
        creates[3..].iter().all(|e| e.contains("glyphs")),
        "{creates:?}"
    );
    for (index, expected) in [
        (0, "q1"), // normal
        (1, "q2"), // hover
        (2, "q3"), // pressed
        (3, "q1"), // the up outside ends the press: normal again
    ] {
        let upto = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.as_str() == "record mark")
            .nth(index)
            .map(|(i, _)| i)
            .expect("a mark");
        let alive = quads(&entries[..upto]);
        assert_eq!(
            in_place(&alive, &["q1", "q2", "q3"], -1_000_000),
            [expected],
            "after mark {index}"
        );
    }
    // `set_text` grows the background (the one in place and the parked ones) and resets the region.
    let after = readout.after_mark(4);
    assert!(
        after.contains(&"set_hit button_1 10 20 60 30".to_owned()),
        "{after:?}"
    );
    assert!(
        after.contains(&"set_rect q1 10 20 60 30".to_owned()),
        "{after:?}"
    );
    assert!(
        after.contains(&"set_rect q2 -1000000 -1000000 60 30".to_owned()),
        "{after:?}"
    );
    assert!(
        after.contains(&"set_rect q3 -1000000 -1000000 60 30".to_owned()),
        "{after:?}"
    );
}

#[test]
fn moving_a_button_moves_its_background_its_glyphs_and_its_region() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}mark()\nb:move(30, 40)\nmark()\nb:move(30, 40)\nmark()"),
        None,
    );
    assert_eq!(
        readout.after_mark(1),
        [
            "set_position q1 30 40",
            "set_position q4 34 44",
            "set_position q5 46 44",
            "set_hit button_1 30 40 32 30",
        ]
    );
    // The same position again repositions the background and rewrites the region but moves no glyph.
    assert_eq!(
        readout.after_mark(2),
        ["set_position q1 30 40", "set_hit button_1 30 40 32 30"]
    );
}

#[test]
fn destroying_a_button_removes_its_quads_and_its_region_and_it_stops_receiving_events() {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}mark()\nb:destroy()\nmark()\nsend(packet(key, {{ev('down', 12, 22)}}), true)\nclicked()\nb:destroy()"
        ),
        None,
    );
    let after = readout.after_mark(1);
    assert_eq!(
        after,
        [
            "set_hit button_1 nil",
            "destroy q1",
            "destroy q2",
            "destroy q3",
            "destroy q4",
            "destroy q5",
        ]
    );
    // After the destroy nothing is registered: the event is ignored (the packet still reports its event and its `over`), and a
    // second destroy does nothing.
    assert_eq!(records(&readout), ["clicks 0"]);
    assert_eq!(
        readout.entries().last().map(String::as_str),
        Some("record clicks 0")
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// The button's states and the dispatcher
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_button_is_hover_when_the_pointer_is_over_it_pressed_on_a_down_and_clicks_once_on_an_up_inside()
{
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}mark()\n\
             send(packet(key, {{}}), true)\nmark()\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\nmark()\nclicked()\n\
             send(packet(key, {{ev('move', 14, 24)}}, false, true), true)\nmark()\n\
             send(packet(key, {{ev('up', 14, 24)}}), true)\nclicked()\nmark()\n\
             send(packet(false, {{}}), true)\nmark()"
        ),
        None,
    );
    // over: hover (the normal background parks, the hover one comes into place)
    assert_eq!(
        readout.after_mark(1),
        ["set_position q1 -1000000 -1000000", "set_position q2 10 20"]
    );
    // down: pressed
    assert_eq!(
        readout.after_mark(2),
        ["set_position q2 -1000000 -1000000", "set_position q3 10 20"]
    );
    // no click yet, and a move while pressed changes nothing on screen
    assert_eq!(readout.after_mark(3), ["record clicks 0"]);
    // up inside: one click, and the look is hover again (the pointer is still over the button)
    assert_eq!(
        readout.after_mark(4),
        [
            "set_position q3 -1000000 -1000000",
            "set_position q2 10 20",
            "record clicks 1",
        ]
    );
    // out: normal
    assert_eq!(
        readout.after_mark(5),
        ["set_position q2 -1000000 -1000000", "set_position q1 10 20"]
    );
}

#[test]
fn an_up_outside_the_rectangle_a_cancel_and_an_up_with_no_down_give_no_click() {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}\n\
             -- up outside (the button spans x 10..42, y 20..50)\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(key, {{ev('up', 42, 22)}}), true)\nclicked()\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(key, {{ev('up', 12, 50)}}), true)\nclicked()\n\
             -- inside, up to the last point before the far corner\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(key, {{ev('up', 41.99, 49.99)}}), true)\nclicked()\n\
             -- cancel, and then an up\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(key, {{ev('cancel', 12, 22)}}), true)\n\
             send(packet(key, {{ev('up', 12, 22)}}), true)\nclicked()\n\
             -- an up with no down\n\
             send(packet(key, {{ev('up', 12, 22)}}), true)\nclicked()"
        ),
        None,
    );
    assert_eq!(
        records(&readout),
        ["clicks 0", "clicks 0", "clicks 1", "clicks 1", "clicks 1"]
    );
}

#[test]
fn hover_is_told_to_the_old_widget_before_the_new_one_and_a_leave_is_told() {
    let readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local function widget(name)
  return {
    on_event = function(_, event) window.record(name .. ":" .. event.kind) end,
    on_hover = function(_, hovered) window.record(name .. ":hover " .. tostring(hovered)) end,
    on_overflow = function() window.record(name .. ":overflow") end,
  }
end
pointer.register("a", widget("a"))
pointer.register("b", widget("b"))
local function step(over)
  window.record("-- " .. tostring(over) .. " changed=" .. tostring(pointer.dispatch({over = over, overflow = false, events = {}, held = false})))
end
step("a")
step("a")
step("b")
step(false)
step(false)
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        [
            "a:hover true",
            "-- a changed=true",
            "-- a changed=false",
            "a:hover false",
            "b:hover true",
            "-- b changed=true",
            "b:hover false",
            "-- false changed=true",
            "-- false changed=false",
        ]
    );
}

#[test]
fn events_go_to_their_keys_in_order_and_a_false_key_or_an_unknown_key_is_ignored() {
    let readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local function widget(name)
  return {
    on_event = function(_, event, eligible) window.record(name .. ":" .. event.kind .. " " .. tostring(eligible)) end,
    on_hover = function() end,
    on_overflow = function() end,
  }
end
pointer.register("a", widget("a"))
pointer.register("b", widget("b"))
local changed = pointer.dispatch({over = false, overflow = false, held = false, events = {
  {kind = "down", key = "a", x = 1, y = 1},
  {kind = "down", key = false, x = 1, y = 1},
  {kind = "scroll", key = "nobody", x = 1, y = 1},
  {kind = "up", key = "b", x = 1, y = 1},
  {kind = "cancel", key = "a", x = 1, y = 1},
}})
window.record("changed " .. tostring(changed))
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        ["a:down true", "b:up true", "a:cancel true", "changed true"]
    );
}

#[test]
fn a_missing_pointer_changes_nothing_and_reports_no_change() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}mark()\nwindow.record(tostring(pointer.dispatch(nil)))\nmark()"),
        None,
    );
    assert_eq!(readout.after_mark(1), ["record false"]);
}

#[test]
fn an_over_only_packet_changes_the_look_and_an_identical_one_makes_no_scene_call() {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}mark()\n\
             send(packet(key, {{}}), true)\nmark()\n\
             send(packet(key, {{}}), false)\nmark()\n\
             send(packet(key, {{}}, false, true), false)\nmark()"
        ),
        None,
    );
    assert_eq!(
        readout.after_mark(1),
        ["set_position q1 -1000000 -1000000", "set_position q2 10 20"]
    );
    // identical: unchanged, no scene call (`held` alone is not a change)
    assert!(readout.after_mark(2).is_empty());
    assert!(readout.after_mark(3).is_empty());
}

#[test]
fn unregistering_the_hovered_widget_clears_the_remembered_hover() {
    let readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local function widget(name)
  return {
    on_event = function() end,
    on_hover = function(_, hovered) window.record(name .. ":hover " .. tostring(hovered)) end,
    on_overflow = function() end,
  }
end
local over_a = {over = "a", overflow = false, events = {}, held = false}
pointer.register("a", widget("first"))
window.record(tostring(pointer.dispatch(over_a)))
pointer.unregister("a")
-- a new widget under the same key hears the hover again, because the dispatcher forgot it
pointer.register("a", widget("second"))
window.record(tostring(pointer.dispatch(over_a)))
window.record(tostring(pointer.dispatch(over_a)))
"#,
        None,
    );
    assert_eq!(
        records(&readout),
        [
            "first:hover true",
            "true",
            "second:hover true",
            "true",
            "false"
        ]
    );
}

#[test]
fn an_overflow_packet_with_no_events_and_the_button_up_releases_every_pressed_widget_without_a_click()
 {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}\
             local other = button.new{{x = 100, y = 20, text = 'No', on_click = function() clicks = clicks + 100 end}}\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(other.key, {{{{kind = 'down', x = 102, y = 22, key = other.key}}}}, false, true), true)\n\
             mark()\n\
             send(packet(false, {{}}, true, false), true)\n\
             clicked()\nmark()"
        ),
        None,
    );
    let entries = readout.entries();
    let marked = entries
        .iter()
        .position(|e| e == "record mark")
        .expect("a mark");
    // both buttons were pressed: each one's pressed quad (q3 and q8) was in place
    let before = quads(&entries[..marked]);
    assert_eq!((before["q3"].0, before["q8"].0), (10, 100));
    // after the packet both are normal again (q1 and q6 in place, the others parked), and nobody clicked
    let after = quads(&entries);
    assert_eq!(
        in_place(&after, &["q1", "q2", "q3", "q6", "q7", "q8"], -1_000_000),
        ["q1", "q6"]
    );
    assert_eq!(records(&readout), ["clicks 0"]);
}

#[test]
fn an_overflow_packet_clicks_nothing_even_with_a_retained_down_and_up() {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}mark()\n\
             send(packet(key, {{ev('down', 12, 22), ev('up', 12, 22)}}, true, false), true)\nclicked()\nmark()"
        ),
        None,
    );
    // pressed, then released (the replay), then hover for the pointer now over the button; no click
    assert_eq!(
        readout.after_mark(1),
        [
            "set_position q1 -1000000 -1000000",
            "set_position q3 10 20",
            "set_position q3 -1000000 -1000000",
            "set_position q1 10 20",
            "set_position q1 -1000000 -1000000",
            "set_position q2 10 20",
            "record clicks 0",
        ]
    );
}

#[test]
fn an_overflow_packet_that_ends_held_releases_the_pressed_button_and_the_next_click_needs_a_fresh_down()
 {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}\
             send(packet(key, {{ev('down', 12, 22)}}, true, true), true)\nclicked()\nmark()\n\
             -- the gesture's later up, in a packet without overflow: the button is not pressed, so it does not click\n\
             send(packet(key, {{ev('up', 12, 22)}}, false, false), true)\nclicked()\n\
             -- a fresh down and up clicks once\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(key, {{ev('up', 12, 22)}}, false, false), true)\nclicked()"
        ),
        None,
    );
    assert_eq!(records(&readout), ["clicks 0", "clicks 0", "clicks 1"]);
    // after the overflow packet the button is not pressed: only the hover quad is in place (the pointer is over it)
    let entries = readout.entries();
    let marked = entries
        .iter()
        .position(|e| e == "record mark")
        .expect("a mark");
    let alive = quads(&entries[..marked]);
    assert_eq!(in_place(&alive, &["q1", "q2", "q3"], -1_000_000), ["q2"]);
}

#[test]
fn the_cancel_of_a_gesture_clears_the_pressed_state_without_a_click() {
    let readout = Readout::script(
        &format!(
            "{GO_BUTTON}send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\nmark()\n\
             send(packet(false, {{ev('cancel', 12, 22)}}), true)\nclicked()\nmark()"
        ),
        None,
    );
    let entries = readout.entries();
    let first = entries
        .iter()
        .position(|e| e == "record mark")
        .expect("a mark");
    assert_eq!(
        in_place(&quads(&entries[..first]), &["q1", "q2", "q3"], -1_000_000),
        ["q3"]
    );
    assert_eq!(
        in_place(&quads(&entries), &["q1", "q2", "q3"], -1_000_000),
        ["q1"]
    );
    assert_eq!(
        records(&readout).last().map(String::as_str),
        Some("clicks 0")
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// main.lua
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn main_lua_registers_its_update_callback() {
    let readout = Readout::new("", None);
    assert!(readout.frame_registered());
}

#[test]
fn the_tooltip_flips_at_the_real_window_width() {
    let flip_x = |width: i64| {
        let mut readout = Readout::new("", None);
        readout.update(width, 600, None);
        readout.deliver_hover(Some(("town", "Alpha", 700, 760, 80)));
        readout
            .entries()
            .into_iter()
            .find(|e| e.starts_with("create q1 ui/font/panel.png"))
            .expect("the tooltip panel")
    };
    // The panel is 60 points wide: at 800 it would end at 824, so it flips to the left of the place (700 - 4 - 60 = 636); at 1200 it
    // fits on the right (760 + 4 = 764).
    assert_eq!(flip_x(800), "create q1 ui/font/panel.png 636 68 60 26");
    assert_eq!(flip_x(1200), "create q1 ui/font/panel.png 764 68 60 26");
}

#[test]
fn a_resize_relayouts_the_readout_and_a_shown_tooltip() {
    let mut readout = Readout::new("", None);
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        1,
        Some(crate::game::slot::PauseReason::Load),
    ));
    readout.deliver_hover(Some(("town", "Alpha", 700, 760, 80)));
    let before = readout.entries().len();
    // The same size again: idle.
    readout.update(800, 600, None);
    assert_eq!(
        readout.entries().len(),
        before,
        "{:?}",
        &readout.entries()[before..]
    );
    // A wider window: the tooltip no longer needs to flip, and the readout is placed again (at the same anchored corner).
    readout.update(1200, 700, None);
    let after = readout.entries()[before..].to_vec();
    assert!(
        after.iter().any(|e| e.starts_with("set_rect q1 8 8 ")),
        "{after:?}"
    );
    assert!(
        after.iter().any(|e| e.starts_with("set_hit readout 8 8 ")),
        "{after:?}"
    );
    assert!(
        after
            .iter()
            .any(|e| e.starts_with("set_rect q") && e.ends_with(" 764 68 60 26")),
        "{after:?}"
    );
}

#[test]
fn an_idle_update_makes_no_scene_call_and_a_change_the_dispatcher_reports_does() {
    let mut readout = Readout::new("", None);
    let pointer = |over: Option<&str>, events: Vec<ConfigValue>| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Float(1.0)),
            ("y".to_owned(), ConfigValue::Float(1.0)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            (
                "over".to_owned(),
                over.map_or(ConfigValue::Bool(false), |key| {
                    ConfigValue::String(key.to_owned())
                }),
            ),
            ("events".to_owned(), ConfigValue::Array(events)),
        ]))
    };
    let before = readout.entries().len();
    readout.update(800, 600, None);
    readout.update(800, 600, Some(pointer(None, Vec::new())));
    readout.update(800, 600, Some(pointer(None, Vec::new())));
    assert_eq!(
        readout.entries().len(),
        before,
        "an idle update touches no scene call"
    );
    // A pointer over a key nobody registered, and then an event for it: the dispatcher is always called, and it finds no widget.
    readout.update(800, 600, Some(pointer(Some("elsewhere"), Vec::new())));
    assert_eq!(readout.entries().len(), before);
}
