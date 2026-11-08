//! The widget library (`scripts/ui/lib/`) run for real in a UI VM over a scene that records its calls: the anchors, units and
//! snapping, the label and the button, the pointer dispatcher, and the library-based `main.lua` (layout against the window's
//! current size and the idle rule).
//!
//! Nothing sleeps: each script runs to the end of its main chunk, or a `frame_update` is posted and awaited.

use std::collections::BTreeMap;

use ck_lua::ConfigValue;

use crate::readout_tests::Readout;

#[path = "../tests/support/time_bar.rs"]
mod time_bar_witness;

#[test]
fn the_window_witness_assertions_validate_captured_output_offline() {
    const CAPTURED: &str = "lua.ui: time-bar speed 1 load\nlua.ui: time-bar speed 3 load\nlua.ui: time-bar speed 3 running\nlua.ui: time-bar speed 3 player\nlua.ui: time-bar gap down\nlua.ui: time-bar gap up\nwindow: window closed\ncamera 0 782598144 782598144\ntime-bar-speed3-injected\ntime-bar-pause1-injected\ntime-bar-pause2-injected\ntime-bar-gap-drag-injected\ntime-bar-gap-drag-observed\n";
    assert!(time_bar_witness::validate(CAPTURED).is_ok());
    assert!(
        time_bar_witness::validate(
            &CAPTURED.replace("time-bar speed 3 running", "time-bar speed 2 running")
        )
        .is_err()
    );
    assert!(
        time_bar_witness::validate(&CAPTURED.replace(
            "camera 0 782598144 782598144",
            "camera 0 782565376 782630912"
        ))
        .is_err()
    );
    assert!(time_bar_witness::script().contains("game.control(control_name)"));
    let fixture = crate::testing::Dir::new();
    time_bar_witness::install_fixture_assets(&fixture.0, &fixture.0);
    assert!(fixture.0.join("lib/button.lua").is_file());
    assert!(fixture.0.join("ui/theme/white.png").is_file());
    assert!(fixture.0.join("ui/theme.toml").is_file());
}

#[test]
fn the_window_witness_observes_down_move_and_up_in_one_real_ui_update() {
    let mut readout = Readout::script(&time_bar_witness::script(), None);
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        3,
        Some(crate::game::slot::PauseReason::Player),
    ));
    let event = |kind: &str, x: i64| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
            ("key".to_owned(), ConfigValue::String("time_bar".to_owned())),
            ("x".to_owned(), ConfigValue::Integer(x)),
            ("y".to_owned(), ConfigValue::Integer(20)),
        ]))
    };
    let pointer = ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Integer(32)),
        ("y".to_owned(), ConfigValue::Integer(20)),
        ("held".to_owned(), ConfigValue::Bool(false)),
        ("overflow".to_owned(), ConfigValue::Bool(false)),
        (
            "over".to_owned(),
            ConfigValue::String("time_bar".to_owned()),
        ),
        (
            "events".to_owned(),
            ConfigValue::Array(vec![event("down", 32), event("move", 40), event("up", 40)]),
        ),
    ]));

    readout.update(800, 600, Some(pointer));

    let entries = readout.entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry == "record time-bar gap down"),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry == "record time-bar gap up"),
        "{entries:?}"
    );
}

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

/// The tints of the frame whose first quad is `first` (11 quads for a frame cut at both corners, in painter order: five fill quads, then
/// six line quads), replayed from the readout's tint log up to its `mark`th mark (all of it when `None`).
fn frame_colours(readout: &Readout, first: usize, mark: Option<usize>) -> Vec<String> {
    let mut colours = vec![String::new(); 11];
    let mut marks = 0;
    for entry in readout.tints() {
        let parts: Vec<&str> = entry.split(' ').collect();
        if entry == "record mark" {
            marks += 1;
            if mark == Some(marks) {
                break;
            }
            continue;
        }
        if parts.len() != 3 || !matches!(parts[0], "create" | "set_tint") {
            continue;
        }
        let Some(number) = parts[1]
            .strip_prefix('q')
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        if (first..first + 11).contains(&number) {
            colours[number - first] = parts[2].to_owned();
        }
    }
    colours
}

/// The colours of a frame filled `fill` and outlined `line`: five fill quads, then six line quads.
fn look(fill: &str, line: &str) -> Vec<String> {
    let mut colours = vec![fill.to_owned(); 5];
    colours.extend(vec![line.to_owned(); 6]);
    colours
}

/// The `set_tint` calls that give quads `first` to `last` the colour.
fn retint(first: usize, last: usize, colour: &str) -> Vec<String> {
    (first..=last)
        .map(|quad| format!("set_tint q{quad} {colour}"))
        .collect()
}

/// The entries without the load and creation of the theme's frame quads (their literals are in `theme_tests`).
fn without_frames(entries: Vec<String>) -> Vec<String> {
    entries
        .into_iter()
        .filter(|entry| !entry.contains("ui/theme/"))
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
-- An event as routing gives it: `inside` says whether the position lay in the button's published rectangle (x 10..42, y 20..50).
local function ev(kind, x, y)
  return {kind = kind, x = x, y = y, key = key, inside = x >= 10 and x < 42 and y >= 20 and y < 50}
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
-- A centred panel 9 UI pixels wide (27 points) in an 800-point window: (800 - 27) / 2 = 386.5, which snaps to 387.
local x, y = ui.anchor("centre", 27, 27, 0, 0, 800, 600)
window.record("centre " .. x .. " " .. y)
panel.new{x = x, y = y, width = 27, height = 27}
"#,
        Some(3),
    );
    assert_eq!(records(&readout), ["P 3", "centre 387 288"]);
    // Each panel is a frame of eleven quads; its first is the top-left tile, at the panel's snapped corner.
    let entries = readout.entries();
    assert_eq!(entries[0], "record P 3");
    assert_eq!(entries[2], "create q1 ui/theme/cut3_tl_fill.png 9 9 12 12");
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.starts_with("create q12 "))
            .collect::<Vec<_>>(),
        ["create q12 ui/theme/cut3_tl_fill.png 387 288 12 12"]
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
local p = panel.new{x = 5, y = 7, width = 31, height = 17}
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
        ["panel 6 8 32 18", "size 24 22", "size 12 22"]
    );
    // The panel is a frame (11 quads, q1 to q11); the label's glyphs follow.
    let entries = readout.entries();
    assert_eq!(entries[1], "create q1 ui/theme/cut3_tl_fill.png 6 8 8 8");
    assert_eq!(
        without_frames(entries)[..8],
        [
            "record panel 6 8 32 18",
            "load ui/font/glyphs/U+0047.png",
            "create q12 ui/font/glyphs/U+0047.png 8 8 10 22",
            "load ui/font/glyphs/U+006F.png",
            "create q13 ui/font/glyphs/U+006F.png 20 8 10 22",
            "record size 24 22",
            "destroy q13",
            "record size 12 22",
        ]
    );
    // `move` to 9, 9 snaps to 10, 10 and repositions the one glyph; `destroy` removes it.
    assert_eq!(
        without_frames(readout.entries())[8..],
        ["set_position q12 10 10", "destroy q12"]
    );
}

#[test]
fn a_buttons_quads_are_literal_rectangles_at_two_points_per_pixel() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}window.record('size ' .. table.concat({{b:size()}}, ' '))"),
        None,
    );
    // 16 by 15 UI pixels: a cut-3 frame at (10, 20) under the text "Go" (12 wide, centred: its left edge is 2 pixels in).
    assert_eq!(
        readout.entries(),
        [
            "load ui/theme/cut3_tl_fill.png",
            "create q1 ui/theme/cut3_tl_fill.png 10 20 8 8",
            "load ui/theme/cut3_br_fill.png",
            "create q2 ui/theme/cut3_br_fill.png 34 42 8 8",
            "load ui/theme/white.png",
            "create q3 ui/theme/white.png 18 22 22 6",
            "create q4 ui/theme/white.png 12 28 28 14",
            "create q5 ui/theme/white.png 12 42 22 6",
            "load ui/theme/cut3_tl_line.png",
            "create q6 ui/theme/cut3_tl_line.png 10 20 8 8",
            "load ui/theme/cut3_br_line.png",
            "create q7 ui/theme/cut3_br_line.png 34 42 8 8",
            "create q8 ui/theme/white.png 18 20 24 2",
            "create q9 ui/theme/white.png 40 22 2 20",
            "create q10 ui/theme/white.png 12 48 22 2",
            "create q11 ui/theme/white.png 10 28 2 22",
            "load ui/font/glyphs/U+0047.png",
            "create q12 ui/font/glyphs/U+0047.png 14 24 10 22",
            "load ui/font/glyphs/U+006F.png",
            "create q13 ui/font/glyphs/U+006F.png 26 24 10 22",
            "set_hit button_1 10 20 32 30",
            "record size 32 30",
        ]
    );
    // The tints: the panel look (body fill, red outline), and the glyphs in the text colour.
    let mut tints: Vec<String> = (1..=5)
        .map(|quad| format!("create q{quad} #15110a"))
        .collect();
    tints.extend((6..=11).map(|quad| format!("create q{quad} #6e130f")));
    tints.extend((12..=13).map(|quad| format!("create q{quad} #ece3cb")));
    tints.push("record size 32 30".to_owned());
    assert_eq!(readout.tints(), tints);
}

#[test]
fn a_buttons_quads_are_literal_rectangles_at_three_points_per_pixel() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}window.record('size ' .. table.concat({{b:size()}}, ' '))"),
        Some(3),
    );
    let entries = readout.entries();
    // 16 by 15 UI pixels at (9, 21): the same frame, every edge three times a UI pixel.
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.starts_with("create")
                || entry.starts_with("set_hit")
                || entry.starts_with("record"))
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "create q1 ui/theme/cut3_tl_fill.png 9 21 12 12",
            "create q2 ui/theme/cut3_br_fill.png 45 54 12 12",
            "create q3 ui/theme/white.png 21 24 33 9",
            "create q4 ui/theme/white.png 12 33 42 21",
            "create q5 ui/theme/white.png 12 54 33 9",
            "create q6 ui/theme/cut3_tl_line.png 9 21 12 12",
            "create q7 ui/theme/cut3_br_line.png 45 54 12 12",
            "create q8 ui/theme/white.png 21 21 36 3",
            "create q9 ui/theme/white.png 54 24 3 30",
            "create q10 ui/theme/white.png 12 63 33 3",
            "create q11 ui/theme/white.png 9 33 3 33",
            "create q12 ui/font/glyphs/U+0047.png 15 27 15 33",
            "create q13 ui/font/glyphs/U+006F.png 33 27 15 33",
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
panel.new{x = 7, y = 9, width = 55, height = 55}:set_rect(11, 13, 47, 49)
local l = label.new{x = 1, y = 2, text = "ab"}
l:move(23, 41)
local x, y = ui.anchor("bottom_right", 47, 45, -8, -8, 801, 601)
panel.new{x = x, y = y, width = 47, height = 45}
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
fn a_buttons_frame_is_eleven_quads_below_its_glyphs_and_its_looks_are_tints_only() {
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
    // Painter order is creation order: the frame's eleven quads precede every glyph.
    let creates: Vec<&String> = entries[..first]
        .iter()
        .filter(|e| e.starts_with("create"))
        .collect();
    assert_eq!(creates.len(), 13);
    assert!(
        creates[..11].iter().all(|e| e.contains("ui/theme/")),
        "{creates:?}"
    );
    assert!(
        creates[11..].iter().all(|e| e.contains("glyphs")),
        "{creates:?}"
    );
    // No look moves a quad: the states differ in tints alone (every `after_mark` here is free of position calls).
    for mark in 1..=3 {
        assert!(
            readout
                .after_mark(mark)
                .iter()
                .all(|entry| !entry.starts_with("set_position")),
            "mark {mark}"
        );
    }
    for (mark, fill, line) in [
        (1, "#15110a", "#6e130f"), // normal
        (2, "#15110a", "#a8261c"), // hover
        (3, "#a8261c", "#a8261c"), // pressed
        (4, "#15110a", "#6e130f"), // the up outside ends the press, and the pointer is not over: normal again
    ] {
        assert_eq!(
            frame_colours(&readout, 1, Some(mark)),
            look(fill, line),
            "at mark {mark}"
        );
    }
    // `set_text` grows the frame (every quad of it) and resets the region: the button is 30 UI pixels wide.
    let after = readout.after_mark(4);
    assert!(
        after.contains(&"set_hit button_1 10 20 60 30".to_owned()),
        "{after:?}"
    );
    assert!(
        after.contains(&"set_rect q9 68 22 2 20".to_owned()),
        "{after:?}"
    );
}

#[test]
fn moving_a_button_moves_its_frame_its_glyphs_and_its_region() {
    let readout = Readout::script(
        &format!("{GO_BUTTON}mark()\nb:move(30, 40)\nmark()\nb:move(30, 40)\nmark()"),
        None,
    );
    assert_eq!(
        readout.after_mark(1),
        [
            "set_rect q1 30 40 8 8",
            "set_rect q2 54 62 8 8",
            "set_rect q3 38 42 22 6",
            "set_rect q4 32 48 28 14",
            "set_rect q5 32 62 22 6",
            "set_rect q6 30 40 8 8",
            "set_rect q7 54 62 8 8",
            "set_rect q8 38 40 24 2",
            "set_rect q9 60 42 2 20",
            "set_rect q10 32 68 22 2",
            "set_rect q11 30 48 2 22",
            "set_position q12 34 44",
            "set_position q13 46 44",
            "set_hit button_1 30 40 32 30",
        ]
    );
    // The same position again moves no quad and no glyph, and rewrites the region.
    assert_eq!(readout.after_mark(2), ["set_hit button_1 30 40 32 30"]);
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
            "destroy q6",
            "destroy q7",
            "destroy q8",
            "destroy q9",
            "destroy q10",
            "destroy q11",
            "destroy q12",
            "destroy q13",
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
    // over: hover (the outline takes the hot colour)
    assert_eq!(readout.after_mark(1), Vec::<String>::new());
    assert_eq!(readout.tints_after_mark(1), retint(6, 11, "#a8261c"));
    // down: pressed (the fill takes the hot colour too; the text colour stays, `pressed_text` being the text colour)
    assert_eq!(readout.after_mark(2), Vec::<String>::new());
    assert_eq!(readout.tints_after_mark(2), retint(1, 5, "#a8261c"));
    // no click yet, and a move while pressed changes nothing on screen
    assert_eq!(readout.after_mark(3), ["record clicks 0"]);
    assert_eq!(readout.tints_after_mark(3), ["record clicks 0"]);
    // up inside: one click, and the look is hover again (the pointer is still over the button)
    assert_eq!(readout.after_mark(4), ["record clicks 1"]);
    assert_eq!(
        readout.tints_after_mark(4),
        [retint(1, 5, "#15110a"), vec!["record clicks 1".to_owned()]].concat()
    );
    // out: normal
    assert_eq!(readout.after_mark(5), Vec::<String>::new());
    assert_eq!(readout.tints_after_mark(5), retint(6, 11, "#6e130f"));
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
    assert_eq!(readout.tints_after_mark(1), retint(6, 11, "#a8261c"));
    // identical: unchanged, no scene call (`held` alone is not a change)
    assert!(readout.after_mark(1).is_empty());
    assert!(readout.after_mark(2).is_empty());
    assert!(readout.after_mark(3).is_empty());
    assert!(readout.tints_after_mark(2).is_empty());
    assert!(readout.tints_after_mark(3).is_empty());
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
    // both buttons were pressed: each frame's fill and line are hot (the first button's quads are q1 to q13, the second's q14 to q26)
    assert_eq!(
        frame_colours(&readout, 1, Some(1)),
        look("#a8261c", "#a8261c")
    );
    assert_eq!(
        frame_colours(&readout, 14, Some(1)),
        look("#a8261c", "#a8261c")
    );
    // after the packet both are normal again, and nobody clicked
    assert_eq!(frame_colours(&readout, 1, None), look("#15110a", "#6e130f"));
    assert_eq!(
        frame_colours(&readout, 14, None),
        look("#15110a", "#6e130f")
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
        readout.tints_after_mark(1),
        [
            retint(1, 5, "#a8261c"),
            // the first press: hover was not yet told, so the line was normal and goes hot with the fill
            retint(6, 11, "#a8261c"),
            // released: back to normal
            retint(1, 5, "#15110a"),
            retint(6, 11, "#6e130f"),
            // then hover (the pointer is over the button)
            retint(6, 11, "#a8261c"),
            vec!["record clicks 0".to_owned()],
        ]
        .concat()
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
    // after the overflow packet the button is not pressed: it has the hover look (the pointer is over it)
    assert_eq!(
        frame_colours(&readout, 1, Some(1)),
        look("#15110a", "#a8261c")
    );
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
    // pressed at the mark; the cancel clears the press, and the pointer is not over the button
    assert_eq!(
        frame_colours(&readout, 1, Some(1)),
        look("#a8261c", "#a8261c")
    );
    assert_eq!(frame_colours(&readout, 1, None), look("#15110a", "#6e130f"));
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
            .find(|e| e.starts_with("create q1 ui/theme/cut3_tl_fill.png"))
            .expect("the tooltip panel")
    };
    // The tooltip's frame is 64 points wide (the name's 28 UI pixels plus 2 on each side): at 800 it would end at 828, so it flips to
    // the left of the place (700 - 4 - 64 = 632); at 1200 it fits on the right (760 + 4 = 764).
    assert_eq!(
        flip_x(800),
        "create q1 ui/theme/cut3_tl_fill.png 632 66 8 8"
    );
    assert_eq!(
        flip_x(1200),
        "create q1 ui/theme/cut3_tl_fill.png 764 66 8 8"
    );
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
    // A wider window: the tooltip no longer needs to flip, and the time bar remains at the same top-left anchor.
    readout.update(1200, 700, None);
    let after = readout.entries()[before..].to_vec();
    // (The bar's frame is where it was: none of its quads is rewritten.)
    assert!(
        after.iter().all(|e| !e.starts_with("set_rect q1 ")),
        "{after:?}"
    );
    assert!(
        after.iter().any(|e| e.starts_with("set_hit time_bar 8 8 ")),
        "{after:?}"
    );
    assert!(
        after
            .iter()
            .any(|e| e.starts_with("set_rect q") && e.ends_with(" 764 66 8 8")),
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

#[test]
fn game_control_sends_the_ten_named_actions_and_refuses_bad_calls_without_sending() {
    let readout = Readout::script(
        r#"
for _, name in ipairs{
  "time_pause", "time_speed_1", "time_speed_2", "time_speed_3", "time_speed_4", "time_speed_5",
  "time_faster", "time_slower", "game_quicksave", "game_quickload"
} do game.control(name) end
for _, args in ipairs{{"bad"}, {12}, {"time_pause", "time_faster"}, {}} do
  local ok, err = pcall(game.control, table.unpack(args))
  window.record(tostring(ok) .. " " .. tostring(err))
end
"#,
        None,
    );
    assert_eq!(
        readout.controls(),
        [
            crate::game::controls::Control::TogglePause,
            crate::game::controls::Control::Speed(1),
            crate::game::controls::Control::Speed(2),
            crate::game::controls::Control::Speed(3),
            crate::game::controls::Control::Speed(4),
            crate::game::controls::Control::Speed(5),
            crate::game::controls::Control::Faster,
            crate::game::controls::Control::Slower,
            crate::game::controls::Control::Quicksave,
            crate::game::controls::Control::Quickload,
        ]
    );
    let refusals = records(&readout);
    assert_eq!(refusals.len(), 4);
    assert!(
        refusals[0].contains("bad"),
        "unknown control error names its argument: {refusals:?}"
    );
    for refusal in &refusals[1..] {
        assert!(
            refusal.contains("game.control") && refusal.contains("name"),
            "argument error names the game.control name argument: {refusals:?}"
        );
    }
}

#[test]
fn the_time_bar_routes_its_buttons_through_game_controls() {
    let mut readout = Readout::new("", None);
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        3,
        Some(crate::game::slot::PauseReason::Player),
    ));
    let entries = readout.entries();
    let broad_hit = entries
        .iter()
        .position(|entry| entry.starts_with("set_hit time_bar "))
        .expect("the bar capture region");
    let first_button = entries
        .iter()
        .position(|entry| entry.starts_with("set_hit button_1 "))
        .expect("the pause button region");
    assert!(broad_hit < first_button, "{entries:?}");
    assert!(
        !entries
            .iter()
            .any(|entry| entry.starts_with("set_hit readout ")),
        "the former readout-only hit region is removed"
    );
    assert!(entries[0].starts_with("load ui/theme/cut3_tl_fill.png"));
    assert!(entries[1].starts_with("create q1 ui/theme/cut3_tl_fill.png 8 8 "));

    let expected = [
        crate::game::controls::Control::TogglePause,
        crate::game::controls::Control::Slower,
        crate::game::controls::Control::Faster,
        crate::game::controls::Control::Speed(1),
        crate::game::controls::Control::Speed(2),
        crate::game::controls::Control::Speed(3),
        crate::game::controls::Control::Speed(4),
        crate::game::controls::Control::Speed(5),
    ];
    for (index, control) in expected.into_iter().enumerate() {
        let key = format!("button_{}", index + 1);
        let hit = readout
            .entries()
            .into_iter()
            .find(|entry| entry.starts_with(&format!("set_hit {key} ")))
            .expect("a button hit region");
        let rect: Vec<i64> = hit
            .split_whitespace()
            .skip(2)
            .map(|part| part.parse().expect("an integer rectangle"))
            .collect();
        let (x, y) = (rect[0] + 1, rect[1] + 1);
        let event = |kind: &str| {
            ConfigValue::Table(BTreeMap::from([
                ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
                ("key".to_owned(), ConfigValue::String(key.clone())),
                ("inside".to_owned(), ConfigValue::Bool(true)),
                ("x".to_owned(), ConfigValue::Integer(x)),
                ("y".to_owned(), ConfigValue::Integer(y)),
            ]))
        };
        let pointer = ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Integer(x)),
            ("y".to_owned(), ConfigValue::Integer(y)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            ("over".to_owned(), ConfigValue::String(key.clone())),
            (
                "events".to_owned(),
                ConfigValue::Array(vec![event("down"), event("up")]),
            ),
        ]));
        readout.update(800, 600, Some(pointer));
        assert_eq!(readout.controls(), [control]);
    }
}

#[test]
fn the_time_bar_geometry_is_literal_at_both_scales_and_readout_width_keeps_buttons_fixed() {
    let cases = [
        (
            2,
            "set_hit time_bar 8 8 568 38",
            [
                "set_hit button_1 12 12 18 30",
                "set_hit button_2 38 12 20 30",
                "set_hit button_3 60 12 20 30",
                "set_hit button_4 88 12 20 30",
                "set_hit button_5 110 12 20 30",
                "set_hit button_6 132 12 20 30",
                "set_hit button_7 154 12 20 30",
                "set_hit button_8 176 12 20 30",
                "set_hit button_9 204 12 20 30",
                "set_hit button_10 226 12 76 30",
            ],
        ),
        (
            3,
            "set_hit time_bar 9 9 852 57",
            [
                "set_hit button_1 15 15 27 45",
                "set_hit button_2 54 15 30 45",
                "set_hit button_3 87 15 30 45",
                "set_hit button_4 129 15 30 45",
                "set_hit button_5 162 15 30 45",
                "set_hit button_6 195 15 30 45",
                "set_hit button_7 228 15 30 45",
                "set_hit button_8 261 15 30 45",
                "set_hit button_9 303 15 30 45",
                "set_hit button_10 336 15 114 45",
            ],
        ),
    ];
    for (scale, panel, expected_hits) in cases {
        let mut readout = Readout::new("", Some(scale));
        readout.deliver(&crate::readout_tests::snapshot(
            14,
            3,
            Some(crate::game::slot::PauseReason::Player),
        ));
        let before = readout.entries();
        assert!(before.contains(&panel.to_owned()), "scale {scale}: {panel}");
        let actual_hits: Vec<String> = before
            .iter()
            .filter(|entry| entry.starts_with("set_hit button_"))
            .cloned()
            .collect();
        assert_eq!(actual_hits, expected_hits);

        let before_resize = before.len();
        readout.deliver(&crate::readout_tests::snapshot(14, 3, None));
        let after = readout.entries();
        let changed = &after[before_resize..];
        let later_hits: Vec<String> = after
            .iter()
            .filter(|entry| entry.starts_with("set_hit button_"))
            .cloned()
            .collect();
        assert_eq!(later_hits, actual_hits, "scale {scale}");
        assert!(
            changed
                .iter()
                .any(|entry| entry.starts_with("set_hit time_bar ")),
            "the panel width follows the readout at scale {scale}: {changed:?}"
        );
    }
}

/// Whether any part of `rect` (in points) lies inside an 800 by 600 point window, which is where `main.lua` assumes the window is
/// until the first update: a quad wholly outside it is never placed.
fn on_screen(rect: Rect) -> bool {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a test window of a few hundred points"
    )]
    let (x, y, w, h) = (rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64);
    x + w > 0.0 && x < 800.0 && y + h > 0.0 && y < 600.0
}

#[test]
fn the_tooltip_places_no_panel_while_hidden_and_exactly_one_when_shown() {
    let mut readout = Readout::new("", None);
    let panel_rects = |readout: &Readout| -> Vec<Rect> {
        let entries = readout.entries();
        let alive = quads(&entries);
        entries
            .iter()
            .filter(|entry| {
                entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
            })
            .map(|entry| entry.split(' ').nth(1).expect("a quad name").to_owned())
            .filter_map(|name| alive.get(&name).copied())
            .collect()
    };
    assert!(
        panel_rects(&readout).is_empty(),
        "no tooltip panel exists before the first hover"
    );
    readout.deliver_hover(Some(("town", "Alpha", 100, 160, 80)));
    let shown = panel_rects(&readout);
    assert_eq!(shown.len(), 1);
    assert!(
        on_screen(shown[0]),
        "the shown tooltip's panel is placed: {shown:?}"
    );
    readout.deliver_hover(None);
    let hidden = panel_rects(&readout);
    assert_eq!(hidden.len(), 1, "the panel is parked, not destroyed");
    assert!(
        !on_screen(hidden[0]),
        "the hidden tooltip's panel is not placed: {hidden:?}"
    );
    assert_eq!(hidden[0].0, -1_000_000_000_000, "parked at ui.HIDDEN");
    readout.deliver_hover(Some(("town", "Alpha", 100, 160, 80)));
    let again = panel_rects(&readout);
    assert!(
        on_screen(again[0]),
        "and placed again when shown: {again:?}"
    );
}
