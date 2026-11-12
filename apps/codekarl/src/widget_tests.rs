//! The widget library (`scripts/ui/lib/`) run for real in a UI VM over a scene that records its calls: the anchors, units and
//! snapping, the label and the button, the pointer dispatcher, and the library-based `main.lua` (layout against the window's
//! current size and the idle rule).
//!
//! Nothing sleeps: each script runs to the end of its main chunk, or a `frame_update` is posted and awaited.

use std::collections::BTreeMap;

use ck_lua::ConfigValue;

use crate::config::TooltipsConfig;
use crate::readout_tests::Readout;

fn pointer_packet(over: &str, x: f64, events: Vec<ConfigValue>) -> ConfigValue {
    ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Float(x)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("held".to_owned(), ConfigValue::Bool(false)),
        ("overflow".to_owned(), ConfigValue::Bool(false)),
        ("over".to_owned(), ConfigValue::String(over.to_owned())),
        ("events".to_owned(), ConfigValue::Array(events)),
    ]))
}

fn pointer_event(kind: &str, key: Option<&str>, functional: Option<bool>) -> ConfigValue {
    let mut fields = BTreeMap::from([
        ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
        ("x".to_owned(), ConfigValue::Float(32.0)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("inside".to_owned(), ConfigValue::Bool(true)),
    ]);
    fields.insert(
        "key".to_owned(),
        key.map_or(ConfigValue::Bool(false), |key| {
            ConfigValue::String(key.to_owned())
        }),
    );
    if let Some(functional) = functional {
        fields.insert("functional".to_owned(), ConfigValue::Bool(functional));
    }
    ConfigValue::Table(fields)
}

#[path = "../tests/support/time_bar.rs"]
pub(crate) mod time_bar_witness;

#[test]
fn the_window_witness_assertions_validate_captured_output_offline() {
    const CAPTURED: &str = "lua.ui: time-bar speed 1 load boxes 1\nlua.ui: time-bar icon play initial\nlua.ui: time-bar speed 3 load boxes 3\nlua.ui: time-bar UI extent 1280 800\nlua.ui: time-bar box down speed_5 inside true\nlua.ui: time-bar box up speed_5 inside true\nlua.ui: time-bar speed 5 load boxes 5\nlua.ui: time-bar speed 3 load boxes 3\nlua.ui: time-bar speed 4 load boxes 4\nlua.ui: time-bar icon pause after\nlua.ui: time-bar box down speed_3 inside true\nlua.ui: time-bar box up speed_3 inside false\nlua.ui: time-bar speed 4 running boxes 4\nwindow: window closed\ncamera 0 782630912 782630912\ntime-bar-speed5-click-injected\ntime-bar-speed3-reset-injected\ntime-bar-fast-forward-injected\ntime-bar-resume-injected\ntime-bar-box-drag-injected\ntime-bar-box-drag-observed-injected\n";
    assert!(time_bar_witness::validate(CAPTURED).is_ok());
    time_bar_witness::captured_failure_and_doctored_output_are_checked_offline();
    let out_of_order = CAPTURED
        .replace("lua.ui: time-bar icon play initial\n", "")
        .replace(
            "lua.ui: time-bar box up speed_5 inside true\n",
            "lua.ui: time-bar box up speed_5 inside true\nlua.ui: time-bar icon play initial\n",
        );
    assert!(time_bar_witness::validate(&out_of_order).is_err());
    assert!(
        time_bar_witness::validate(&format!("{CAPTURED}camera 0 782630912 782630912\n")).is_err()
    );
    assert!(
        time_bar_witness::validate(
            &CAPTURED.replace("time-bar speed 4 running", "time-bar speed 3 running")
        )
        .is_err()
    );
    assert!(
        time_bar_witness::validate(&CAPTURED.replace(
            "camera 0 782630912 782630912",
            "camera 0 782598144 782663680"
        ))
        .is_err()
    );
    assert!(time_bar_witness::script().contains("game.control(\"time_faster\")"));
    let fixture = crate::testing::Dir::new();
    time_bar_witness::install_fixture_assets(&fixture.0, &fixture.0);
    assert!(fixture.0.join("lib/button.lua").is_file());
    assert!(fixture.0.join("ui/theme/white.png").is_file());
    assert!(fixture.0.join("ui/theme.toml").is_file());
}

#[test]
fn the_window_witness_observes_down_move_and_up_in_one_real_ui_update() {
    let mut readout = Readout::script(&time_bar_witness::script());
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        3,
        Some(crate::game::slot::PauseReason::Player),
    ));
    let event = |kind: &str, x: i64, inside: bool| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
            ("key".to_owned(), ConfigValue::String("speed_3".to_owned())),
            ("x".to_owned(), ConfigValue::Integer(x)),
            ("y".to_owned(), ConfigValue::Integer(20)),
            ("inside".to_owned(), ConfigValue::Bool(inside)),
        ]))
    };
    let pointer = ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Integer(32)),
        ("y".to_owned(), ConfigValue::Integer(20)),
        ("held".to_owned(), ConfigValue::Bool(false)),
        ("overflow".to_owned(), ConfigValue::Bool(false)),
        ("over".to_owned(), ConfigValue::String("speed_3".to_owned())),
        (
            "events".to_owned(),
            ConfigValue::Array(vec![
                event("down", 32, true),
                event("move", 40, false),
                event("up", 40, false),
            ]),
        ),
    ]));

    readout.update(800, 600, Some(pointer));

    let entries = readout.entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry == "record time-bar box down speed_3 inside true"),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry == "record time-bar box up speed_3 inside false"),
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

/// The shipped font fixture identifies each glyph by code point; pair those glyphs with their latest scene tint.
fn tooltip_glyphs(readout: &Readout) -> Vec<(char, Rect, String)> {
    let entries = readout.entries();
    let alive = quads(&entries);
    let tints: BTreeMap<String, String> = readout
        .tints()
        .iter()
        .filter_map(|entry| {
            let mut fields = entry.split_whitespace();
            let operation = fields.next()?;
            if !matches!(operation, "create" | "set_tint") {
                return None;
            }
            Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
        })
        .collect();
    entries
        .iter()
        .filter_map(|entry| {
            let mut fields = entry.split_whitespace();
            if fields.next()? != "create" {
                return None;
            }
            let quad = fields.next()?.to_owned();
            let texture = fields.next()?;
            let code = texture
                .strip_prefix("ui/font/")?
                .split_once("/U+")?
                .1
                .strip_suffix(".png")?;
            let character = char::from_u32(u32::from_str_radix(code, 16).ok()?)?;
            let rect = alive.get(&quad).copied()?;
            Some((
                character,
                rect,
                tints.get(&quad).cloned().unwrap_or_default(),
            ))
        })
        .collect()
}

fn assert_tooltip_text_tint(readout: &Readout, text: &str, colour: &str) {
    let rectangles = tooltip_text_rects(readout, text);
    let glyphs = tooltip_glyphs(readout);
    let found = rectangles.iter().any(|rectangles| {
        rectangles.iter().all(|rect| {
            glyphs
                .iter()
                .find(|(_, glyph_rect, _)| glyph_rect == rect)
                .is_some_and(|(_, _, tint)| tint == colour)
        })
    });
    assert!(
        found,
        "{text:?} is not rendered in {colour}; entries: {:?}",
        readout.entries()
    );
}

fn tooltip_text_rects(readout: &Readout, text: &str) -> Vec<Vec<Rect>> {
    let wanted: Vec<char> = text.chars().collect();
    let mut glyphs = tooltip_glyphs(readout);
    glyphs.sort_by_key(|(character, rect, _)| (rect.1, rect.0, *character));
    glyphs
        .windows(wanted.len())
        .filter_map(|window| {
            let row_y = window[0].1.1;
            (window
                .iter()
                .map(|(character, _, _)| *character)
                .eq(wanted.iter().copied())
                && window.iter().all(|(_, rect, _)| rect.1 == row_y))
            .then(|| window.iter().map(|(_, rect, _)| *rect).collect())
        })
        .collect()
}

fn tooltip_scroll_readout(lock: &'static str, mixed_fonts: bool) -> Readout {
    let font_choice = if mixed_fonts {
        "index % 2 == 0 and 'tiny' or 'small'"
    } else {
        "'small'"
    };
    Readout::script_with_tooltips(
        &format!(
            r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("long_target", {{on_event = function() end, on_hover = function() end, on_overflow = function() end}})
render.set_hit("long_target", {{{{x = 20, y = 20, width = 80, height = 24}}}})
local lines = {{}}
for index = 1, 20 do
  lines[index] = {{{{text = string.format("row%02d", index), font = {font_choice}, tooltip = function() return {{"child"}} end}}}}
end
tooltip.attach("long_target", function() return lines end)
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  if update.pointer.over == "refresh_short" then
    for index = 20, 16, -1 do lines[index] = nil end
    tooltip.refresh()
  end
  tooltip.update(update)
end)
"#
        ),
        TooltipsConfig {
            delay: 0.0,
            lock,
            lock_timer: 1.0,
        },
    )
}

fn long_tooltip_pointer_event(
    kind: &str,
    key: &str,
    amount: Option<f64>,
    unit: Option<&str>,
) -> ConfigValue {
    let mut fields = BTreeMap::from([
        ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
        ("key".to_owned(), ConfigValue::String(key.to_owned())),
        ("x".to_owned(), ConfigValue::Float(32.0)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("inside".to_owned(), ConfigValue::Bool(true)),
    ]);
    if let Some(amount) = amount {
        fields.insert("amount".to_owned(), ConfigValue::Float(amount));
    }
    if let Some(unit) = unit {
        fields.insert("unit".to_owned(), ConfigValue::String(unit.to_owned()));
    }
    ConfigValue::Table(fields)
}

fn tooltip_scroll_thumb(readout: &Readout) -> Rect {
    let tints: BTreeMap<String, String> = readout
        .tints()
        .iter()
        .filter_map(|entry| {
            let mut fields = entry.split_whitespace();
            let operation = fields.next()?;
            if !matches!(operation, "create" | "set_tint") {
                return None;
            }
            Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
        })
        .collect();
    let alive = quads(&readout.entries());
    alive
        .into_iter()
        .find_map(|(quad, rect)| {
            (rect.2 == 6 && rect.3 == 126 && tints.get(&quad).is_some_and(|tint| tint == "#ece3cb"))
                .then_some(rect)
        })
        .expect("the 63-unit scroll thumb")
}

fn tooltip_panel_rect(readout: &Readout) -> Option<Rect> {
    let entries = readout.entries();
    let alive = quads(&entries);
    let panel_quads: Vec<Rect> = entries
        .iter()
        .filter(|entry| entry.starts_with("create ") && entry.contains("ui/theme/cut3_"))
        .filter_map(|entry| entry.split_whitespace().nth(1))
        .filter_map(|name| alive.get(name).copied())
        .collect();
    let left = panel_quads.iter().map(|rect| rect.0).min()?;
    let top = panel_quads.iter().map(|rect| rect.1).min()?;
    let right = panel_quads.iter().map(|rect| rect.0 + rect.2).max()?;
    let bottom = panel_quads.iter().map(|rect| rect.1 + rect.3).max()?;
    Some((left, top, right - left, bottom - top))
}

fn tooltip_panel_count(readout: &Readout) -> usize {
    let entries = readout.entries();
    let alive = quads(&entries);
    entries
        .iter()
        .filter(|entry| entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png"))
        .filter_map(|entry| entry.split_whitespace().nth(1))
        .filter(|name| alive.contains_key(*name))
        .count()
}

fn assert_mixed_tooltip_rows(readout: &Readout, first: usize, panel_y: i64) {
    for index in first..first + 12 {
        let label = format!("row{index:02}");
        let rects = tooltip_text_rects(readout, &label)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("visible row {label} has glyphs"));
        let expected_y = panel_y + 4 + i64::try_from(index - first).unwrap() * 18;
        let expected_height = if index % 2 == 0 { 16 } else { 18 };
        assert!(
            rects
                .iter()
                .all(|rect| rect.1 == expected_y && rect.3 == expected_height),
            "{label} uses its fixed mixed-font row: {rects:?}"
        );
        assert!(
            rects.iter().all(|rect| rect.1 + rect.3 <= panel_y + 220),
            "{label} stays inside the 112-pixel panel"
        );
    }
}

fn current_hits(entries: &[String], prefix: &str) -> BTreeMap<String, String> {
    let mut hits = BTreeMap::new();
    for entry in entries
        .iter()
        .filter_map(|entry| entry.strip_prefix("set_hit "))
    {
        let mut fields = entry.split_whitespace();
        let Some(key) = fields.next() else { continue };
        if !key.starts_with(prefix) {
            continue;
        }
        let rect = fields.collect::<Vec<_>>().join(" ");
        if rect == "nil" {
            hits.remove(key);
        } else {
            hits.insert(key.to_owned(), rect);
        }
    }
    hits
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}window.record('size ' .. table.concat({{b:size()}}, ' '))"
    ));
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
fn every_quad_and_hit_edge_is_a_multiple_of_the_pixel_scale() {
    // A UI pixel is two units: every coordinate the library creates, moves, resizes or publishes as a hit region is a multiple of 2, off-grid
    // inputs included.
    let scale = 2;
    {
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\nsend(packet(key, {{}}), true)\nmark()\nsend(packet(key, {{ev('down', 12, 22)}}, false, true), true)\nmark()\nsend(packet(false, {{ev('up', 500, 500)}}), true)\nmark()\nb:set_text('Hello')\nmark()"
    ));
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\nb:move(30, 40)\nmark()\nb:move(30, 40)\nmark()"
    ));
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\nb:destroy()\nmark()\nsend(packet(key, {{ev('down', 12, 22)}}), true)\nclicked()\nb:destroy()"
    ));
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\n\
             send(packet(key, {{}}), true)\nmark()\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\nmark()\nclicked()\n\
             send(packet(key, {{ev('move', 14, 24)}}, false, true), true)\nmark()\n\
             send(packet(key, {{ev('up', 14, 24)}}), true)\nclicked()\nmark()\n\
             send(packet(false, {{}}), true)\nmark()"
    ));
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
    let readout = Readout::script(&format!(
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
    ));
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
    );
    assert_eq!(
        records(&readout),
        ["a:down true", "b:up true", "a:cancel true", "changed true"]
    );
}

#[test]
fn a_missing_pointer_changes_nothing_and_reports_no_change() {
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\nwindow.record(tostring(pointer.dispatch(nil)))\nmark()"
    ));
    assert_eq!(readout.after_mark(1), ["record false"]);
}

#[test]
fn an_over_only_packet_changes_the_look_and_an_identical_one_makes_no_scene_call() {
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\n\
             send(packet(key, {{}}), true)\nmark()\n\
             send(packet(key, {{}}), false)\nmark()\n\
             send(packet(key, {{}}, false, true), false)\nmark()"
    ));
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}\
             local other = button.new{{x = 100, y = 20, text = 'No', on_click = function() clicks = clicks + 100 end}}\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(other.key, {{{{kind = 'down', x = 102, y = 22, key = other.key}}}}, false, true), true)\n\
             mark()\n\
             send(packet(false, {{}}, true, false), true)\n\
             clicked()\nmark()"
    ));
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}mark()\n\
             send(packet(key, {{ev('down', 12, 22), ev('up', 12, 22)}}, true, false), true)\nclicked()\nmark()"
    ));
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
    let readout = Readout::script(&format!(
        "{GO_BUTTON}\
             send(packet(key, {{ev('down', 12, 22)}}, true, true), true)\nclicked()\nmark()\n\
             -- the gesture's later up, in a packet without overflow: the button is not pressed, so it does not click\n\
             send(packet(key, {{ev('up', 12, 22)}}, false, false), true)\nclicked()\n\
             -- a fresh down and up clicks once\n\
             send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\n\
             send(packet(key, {{ev('up', 12, 22)}}, false, false), true)\nclicked()"
    ));
    assert_eq!(records(&readout), ["clicks 0", "clicks 0", "clicks 1"]);
    // after the overflow packet the button is not pressed: it has the hover look (the pointer is over it)
    assert_eq!(
        frame_colours(&readout, 1, Some(1)),
        look("#15110a", "#a8261c")
    );
}

#[test]
fn the_cancel_of_a_gesture_clears_the_pressed_state_without_a_click() {
    let readout = Readout::script(&format!(
        "{GO_BUTTON}send(packet(key, {{ev('down', 12, 22)}}, false, true), true)\nmark()\n\
             send(packet(false, {{ev('cancel', 12, 22)}}), true)\nclicked()\nmark()"
    ));
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
    let readout = Readout::new("");
    assert!(readout.frame_registered());
}

#[test]
fn map_hover_uses_the_shared_location_tooltip_and_the_game_place_name() {
    let mut readout = Readout::new(crate::readout_tests::PLACE_TOOLTIP_TEST_FIXTURE);
    readout.deliver(&crate::readout_tests::snapshot(14, 1, None));
    readout.deliver_hover(Some(("town", "map hover name", 100, 120, 80)));
    for _ in 0..24 {
        readout.update(800, 600, None);
    }
    assert_tooltip_text_tint(&readout, "Alpha", "#c9932e");
    let title = tooltip_text_rects(&readout, "Alpha").remove(0);
    let population = tooltip_text_rects(&readout, "population").remove(0);
    assert!(
        population[0].1 > title[0].1,
        "the population is the second tooltip line"
    );
    assert!(tooltip_text_rects(&readout, "map hover name").is_empty());
}

#[test]
fn the_tooltip_flips_at_the_real_window_width() {
    let flip_x = |width: i64| {
        let mut readout = Readout::script(
            r#"
local tooltip = require("lib.tooltip")
local hover = {id = "town", name = "Alpha", left = 700, right = 760, y = 80}
tooltip.map_hover(hover, function() return {hover.name} end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
        );
        readout.update(width, 600, None);
        for _ in 0..20 {
            readout.update(width, 600, None);
        }
        let entries = readout.entries();
        entries
            .into_iter()
            .rev()
            .find(|e| e.starts_with("set_rect q1 "))
            .expect("the tooltip panel")
    };
    let left = flip_x(800);
    let right = flip_x(1200);
    let x = |entry: &str| {
        entry
            .split_whitespace()
            .nth(2)
            .unwrap()
            .parse::<i64>()
            .unwrap()
    };
    assert!(
        x(&left) < 700,
        "tooltip flips left when the right side does not fit: {left}"
    );
    assert!(
        x(&right) >= 760,
        "tooltip stays to the right when it fits: {right}"
    );
}

#[test]
fn a_map_tooltip_clamps_up_from_the_bottom_edge() {
    let mut readout = Readout::script(
        r#"
local tooltip = require("lib.tooltip")
local hover = {id = "town", name = "Alpha", left = 40, right = 80, y = 590}
tooltip.map_hover(hover, function() return {hover.name} end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
    );
    for _ in 0..20 {
        readout.update(800, 600, None);
    }
    let entries = readout.entries();
    let rect = entries
        .iter()
        .rev()
        .find(|entry| entry.starts_with("set_rect q1 "))
        .expect("tooltip panel rect");
    let values: Vec<i64> = rect
        .split_whitespace()
        .skip(2)
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(values.len(), 4);
    assert!(
        values[1] + values[3] <= 600,
        "bottom placement fits the view: {rect}"
    );
}

#[test]
fn a_resize_relayouts_the_readout_and_a_shown_tooltip() {
    let mut readout = Readout::new(crate::readout_tests::PLACE_TOOLTIP_TEST_FIXTURE);
    // The shipped script opens at 1280 by 800 here (no screen snapshot); this case starts from a window of 800 by 600.
    readout.update(800, 600, None);
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        1,
        Some(crate::game::slot::PauseReason::Load),
    ));
    readout.deliver_hover(Some(("town", "Alpha", 700, 760, 80)));
    for _ in 0..20 {
        readout.update(800, 600, None);
    }
    let before = readout.entries().len();
    // The same size again: idle.
    readout.update(800, 600, None);
    assert!(
        readout.entries()[before..]
            .iter()
            .all(|entry| !entry.starts_with("set_rect q1 ") && entry != "destroy q1"),
        "resize keeps the tooltip at its opening rectangle: {:?}",
        &readout.entries()[before..]
    );
    // The tooltip stays where it opened even when a wider window would fit it on the other side.
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
}

#[test]
fn an_idle_update_makes_no_scene_call_and_a_change_the_dispatcher_reports_does() {
    let mut readout = Readout::new("");
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
    let mut readout = Readout::new("");
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
        .expect("the rewind button region");
    assert!(broad_hit < first_button, "{entries:?}");
    assert!(
        !entries
            .iter()
            .any(|entry| entry.starts_with("set_hit readout ")),
        "the former readout-only hit region is removed"
    );
    assert!(entries[0].starts_with("load ui/theme/cut6_tl_fill.png"));
    assert!(entries[1].starts_with("create q1 ui/theme/cut6_tl_fill.png 8 8 "));
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.starts_with("set_hit button_"))
            .count(),
        9,
        "three transports, Help, the two ledger tabs and the three map-mode buttons: {entries:?}"
    );
    assert!(
        !entries.iter().any(|entry| entry.contains("time_speed_")),
        "speed is no longer represented by buttons"
    );

    let expected = [
        crate::game::controls::Control::Slower,
        crate::game::controls::Control::TogglePause,
        crate::game::controls::Control::Faster,
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
#[expect(
    clippy::too_many_lines,
    reason = "the literal checks form one layout contract"
)]
fn the_time_bar_and_places_bar_follow_the_literal_layout() {
    {
        let scale = 2;
        let mut readout = Readout::new("");
        readout.update(800, 600, None);
        readout.deliver(&crate::readout_tests::snapshot(
            14,
            3,
            Some(crate::game::slot::PauseReason::Player),
        ));
        let before = readout.entries();
        let time_bar = before
            .iter()
            .find(|entry| entry.starts_with("set_hit time_bar "))
            .expect("time bar region");
        let time_rect: Vec<i64> = time_bar
            .split_whitespace()
            .skip(2)
            .map(|v| v.parse().unwrap())
            .collect();
        let margin = if scale == 2 { 8 } else { 9 };
        assert_eq!(
            (time_rect[0], time_rect[1], time_rect[3]),
            (margin, margin, 19 * scale)
        );
        for (index, (x, width)) in [(5, 13), (19, 11), (31, 13), (90, 10)]
            .into_iter()
            .enumerate()
        {
            let key = format!("button_{}", index + 1);
            let entry = before
                .iter()
                .find(|line| line.starts_with(&format!("set_hit {key} ")))
                .unwrap();
            let rect: Vec<i64> = entry
                .split_whitespace()
                .skip(2)
                .map(|v| v.parse().unwrap())
                .collect();
            assert_eq!(
                rect,
                [
                    margin + x * scale,
                    margin + 2 * scale,
                    width * scale,
                    15 * scale
                ],
                "scale {scale}, {key}"
            );
        }
        let mut speed_rects = Vec::new();
        for index in 1..=5 {
            let key = format!("speed_{index}");
            let entry = before
                .iter()
                .find(|line| line.starts_with(&format!("set_hit {key} ")))
                .unwrap();
            let rect: Vec<i64> = entry
                .split_whitespace()
                .skip(2)
                .map(|part| part.parse().unwrap())
                .collect();
            let box_index = index - 1;
            assert_eq!(
                rect,
                [
                    margin + (47 + 8 * box_index) * scale,
                    margin + 2 * scale,
                    8 * scale,
                    15 * scale,
                ],
                "P={scale}, {key}"
            );
            speed_rects.push(rect);
        }
        for adjacent in speed_rects.windows(2) {
            assert_eq!(adjacent[0][0] + adjacent[0][2], adjacent[1][0]);
        }
        let place_bar = before
            .iter()
            .find(|entry| entry.starts_with("set_hit places_bar "))
            .unwrap();
        let place_rect: Vec<i64> = place_bar
            .split_whitespace()
            .skip(2)
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(
            (place_rect[1], place_rect[3], place_rect[0] + place_rect[2]),
            (margin, 19 * scale, 800 - 8)
        );
        let places_hit = before
            .iter()
            .position(|entry| entry.starts_with("set_hit places_bar "))
            .unwrap();
        let places_button = before
            .iter()
            .position(|entry| entry.starts_with("set_hit button_5 "))
            .unwrap();
        assert!(
            places_hit < places_button,
            "the broad region precedes its button: {before:?}"
        );
        let places_button_entry = &before[places_button];
        let button_rect: Vec<i64> = places_button_entry
            .split_whitespace()
            .skip(2)
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(button_rect[0], place_rect[0] + 5 * scale);
        assert_eq!(button_rect[1], place_rect[1] + 2 * scale);

        let before_resize = before.len();
        readout.update(1200, 700, None);
        let after = readout.entries();
        let changed = &after[before_resize..];
        let moved_places = changed
            .iter()
            .find(|entry| entry.starts_with("set_hit places_bar "))
            .unwrap();
        let moved_rect: Vec<i64> = moved_places
            .split_whitespace()
            .skip(2)
            .map(|v| v.parse().unwrap())
            .collect();
        let right = if scale == 2 { 1200 - 8 } else { 1191 };
        assert_eq!(moved_rect[0] + moved_rect[2], right);
        assert!(
            changed
                .iter()
                .any(|entry| entry.starts_with("set_hit button_5 ")),
            "the Places button follows its bar at scale {scale}: {changed:?}"
        );
        let fixed_time_buttons: Vec<String> = after
            .iter()
            .filter(|entry| (1..=4).any(|i| entry.starts_with(&format!("set_hit button_{i} "))))
            .cloned()
            .collect();
        let original_time_buttons: Vec<String> = before
            .iter()
            .filter(|entry| (1..=4).any(|i| entry.starts_with(&format!("set_hit button_{i} "))))
            .cloned()
            .collect();
        assert_eq!(fixed_time_buttons, original_time_buttons);
    }
}

#[test]
fn a_duplicate_icon_name_is_refused_before_anything_is_created_and_a_unique_lists_destroy_removes_every_icon_quad()
 {
    let readout = Readout::script(
        r#"
local button = require("lib.button")
window.record("mark")
local ok, message = pcall(button.new, {x = 10, y = 20, kind = "time_bar", icons = {"pause", "play", "pause"}})
window.record(tostring(ok) .. " " .. tostring(message):gsub("^.-button.new", "button.new", 1))
window.record("mark")
local created = button.new{x = 10, y = 20, kind = "time_bar", icons = {"pause", "play"}}
window.record("mark")
created:destroy()
window.record("mark")
"#,
    );
    // The refusal names the duplicate and makes no scene call at all: no quad, no region, no texture load.
    assert!(
        readout
            .after_mark(1)
            .iter()
            .any(|entry| entry == "record false button.new: icon `pause` is named twice in icons"),
        "{:?}",
        readout.after_mark(1)
    );
    assert!(
        readout
            .after_mark(1)
            .iter()
            .all(|entry| entry.starts_with("record ")),
        "a refused button makes no scene call: {:?}",
        readout.after_mark(1)
    );
    // A unique list creates its frame and both icons (and the region); destroying it removes every one of those quads.
    let entries = readout.entries();
    let created_quads: Vec<String> = readout
        .after_mark(2)
        .iter()
        .filter(|entry| entry.starts_with("create "))
        .map(|entry| entry.split(' ').nth(1).expect("a quad").to_owned())
        .collect();
    assert!(
        created_quads.len() >= 13,
        "the frame's eleven quads and two icons: {created_quads:?} in {entries:?}"
    );
    let mut destroyed: Vec<String> = readout
        .after_mark(3)
        .iter()
        .filter_map(|entry| entry.strip_prefix("destroy ").map(str::to_owned))
        .collect();
    let mut wanted = created_quads;
    wanted.sort();
    destroyed.sort();
    assert_eq!(destroyed, wanted, "every quad the button made is destroyed");
}

#[test]
fn icon_buttons_precreate_all_icons_and_set_icon_only_retints_the_two_quads() {
    let readout = Readout::script(
        r#"
local button = require("lib.button")
local control = button.new{x = 10, y = 20, kind = "time_bar", icons = {"pause", "play"}}
window.record("mark")
control:set_icon("play")
window.record("mark")
control:set_icon("play")
window.record("mark")
local ok, message = pcall(control.set_icon, control, "rewind")
window.record(tostring(ok) .. " " .. tostring(message))
local ok2 = pcall(button.new, {x = 0, y = 0, kind = "time_bar", icons = {"pause", "rewind"}})
window.record(tostring(ok2))
"#,
    );
    let created: Vec<_> = readout
        .entries()
        .iter()
        .filter(|entry| entry.starts_with("create ") && entry.contains("ui/theme/icon_"))
        .cloned()
        .collect();
    assert_eq!(created.len(), 2, "{created:?}");
    assert!(created[0].contains("icon_pause.png"));
    assert!(created[1].contains("icon_play.png"));
    assert!(created[0].ends_with(" 14 24 14 22"), "{created:?}");
    assert!(created[1].ends_with(" 14 24 14 22"), "{created:?}");
    let ids: Vec<String> = created
        .iter()
        .map(|entry| entry.split_whitespace().nth(1).unwrap().to_owned())
        .collect();
    let tints = readout.tints();
    let first_mark = tints
        .iter()
        .position(|entry| entry == "record mark")
        .unwrap();
    let second_mark = tints
        .iter()
        .skip(first_mark + 1)
        .position(|entry| entry == "record mark")
        .unwrap()
        + first_mark
        + 1;
    let third_mark = tints
        .iter()
        .skip(second_mark + 1)
        .position(|entry| entry == "record mark")
        .unwrap()
        + second_mark
        + 1;
    assert_eq!(
        &tints[first_mark + 1..second_mark],
        [
            format!("set_tint {} #00000000", ids[0]),
            format!("set_tint {} #ece3cb", ids[1])
        ]
    );
    assert!(
        tints[second_mark + 1..third_mark].is_empty(),
        "same icon is a no-op"
    );
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry.contains("icon `rewind` is not in icons"))
    );
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry == "record false"),
        "unequal icon widths are refused before drawing"
    );
}

#[test]
fn speed_boxes_follow_simulation_state_and_only_changed_boxes_are_retinted() {
    {
        let scale = 2;
        for (speed, paused) in [
            (1, None),
            (3, None),
            (5, None),
            (3, Some(crate::game::slot::PauseReason::Player)),
        ] {
            let mut readout = Readout::new("");
            readout.deliver(&crate::readout_tests::snapshot(14, speed, paused));
            let expected: Vec<String> = (1..=5)
                .map(|index| {
                    if index <= speed {
                        "#a8261c"
                    } else {
                        "#00000000"
                    }
                    .to_owned()
                })
                .collect();
            assert_eq!(
                speed_inner_tints(&readout, scale),
                expected,
                "P={scale}, speed={speed}, paused={paused:?}"
            );
            let outlines: Vec<_> = readout
                .entries()
                .into_iter()
                .filter(|entry| {
                    entry.starts_with("create ") && entry.contains("ui/theme/speed_box.png")
                })
                .collect();
            assert_eq!(outlines.len(), 5, "{outlines:?}");
            assert!(outlines.iter().all(|entry| entry.ends_with(&format!(
                " {} {}",
                6 * scale,
                11 * scale
            ))));
            assert!(outlines.iter().all(|entry| {
                let quad = entry.split_whitespace().nth(1).unwrap();
                readout
                    .tints()
                    .iter()
                    .any(|tint| tint == &format!("create {quad} #ece3cb"))
            }));
        }
    }

    let mut readout = Readout::new("");
    readout.deliver(&crate::readout_tests::snapshot(14, 3, None));
    let before_pause = readout.tints().len();
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        3,
        Some(crate::game::slot::PauseReason::Player),
    ));
    let pause_changes = &readout.tints()[before_pause..];
    assert_eq!(
        pause_changes.len(),
        2,
        "pause/play precreated icons are retinted: {pause_changes:?}"
    );
    assert_eq!(
        speed_inner_tints(&readout, 2),
        ["#a8261c", "#a8261c", "#a8261c", "#00000000", "#00000000"]
    );
    let before_speed = readout.tints().len();
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        4,
        Some(crate::game::slot::PauseReason::Player),
    ));
    let changed = &readout.tints()[before_speed..];
    assert_eq!(
        changed.len(),
        1,
        "one newly filled box while paused: {changed:?}"
    );
    assert!(changed[0].ends_with(" #a8261c"), "{changed:?}");
    assert_eq!(
        speed_inner_tints(&readout, 2),
        ["#a8261c", "#a8261c", "#a8261c", "#a8261c", "#00000000"]
    );
}

fn speed_inner_tints(readout: &Readout, scale: i64) -> Vec<String> {
    let margin = if scale == 2 { 8 } else { 9 };
    let mut tints = BTreeMap::new();
    for entry in readout.tints() {
        let mut fields = entry.split_whitespace();
        if let Some("create" | "set_tint") = fields.next() {
            tints.insert(
                fields.next().unwrap().to_owned(),
                fields.next().unwrap().to_owned(),
            );
        }
    }
    let mut inner = vec![String::new(); 5];
    for entry in readout
        .entries()
        .iter()
        .filter(|entry| entry.contains("ui/theme/white.png"))
    {
        let fields: Vec<_> = entry.split_whitespace().collect();
        if fields.len() != 7
            || fields[5] != (2 * scale).to_string()
            || fields[6] != (7 * scale).to_string()
        {
            continue;
        }
        let x: i64 = fields[3].parse().unwrap();
        let y: i64 = fields[4].parse().unwrap();
        if let Some(index) = (0_usize..5).find(|index| {
            let index = i64::try_from(*index).unwrap();
            x == margin + (50 + index * 8) * scale && y == margin + 6 * scale
        }) {
            inner[index] = tints[fields[1]].clone();
        }
    }
    inner
}

#[test]
fn speed_boxes_click_their_named_speed_and_release_without_clicking_on_cancel_or_overflow() {
    use crate::game::controls::Control;

    for index in 1..=5 {
        let mut readout = Readout::new("");
        readout.deliver(&crate::readout_tests::snapshot(14, 3, None));
        let key = format!("speed_{index}");
        let hit = readout
            .entries()
            .into_iter()
            .find(|entry| entry.starts_with(&format!("set_hit {key} ")))
            .expect("the speed hit region");
        let rect: Vec<i64> = hit
            .split_whitespace()
            .skip(2)
            .map(|part| part.parse().expect("integer hit rectangle"))
            .collect();
        let (x, y) = (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2);
        let event = |kind: &str, inside: bool| {
            ConfigValue::Table(BTreeMap::from([
                ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
                ("key".to_owned(), ConfigValue::String(key.clone())),
                ("inside".to_owned(), ConfigValue::Bool(inside)),
                ("x".to_owned(), ConfigValue::Integer(x)),
                ("y".to_owned(), ConfigValue::Integer(y)),
            ]))
        };
        let packet = |events: Vec<ConfigValue>, overflow: bool| {
            ConfigValue::Table(BTreeMap::from([
                ("x".to_owned(), ConfigValue::Integer(x)),
                ("y".to_owned(), ConfigValue::Integer(y)),
                ("held".to_owned(), ConfigValue::Bool(false)),
                ("overflow".to_owned(), ConfigValue::Bool(overflow)),
                ("over".to_owned(), ConfigValue::String(key.clone())),
                ("events".to_owned(), ConfigValue::Array(events)),
            ]))
        };
        readout.update(
            800,
            600,
            Some(packet(vec![event("down", true), event("up", true)], false)),
        );
        assert_eq!(readout.controls(), [Control::Speed(index)]);
    }

    let mut readout = Readout::new("");
    readout.deliver(&crate::readout_tests::snapshot(14, 3, None));
    let key = "speed_4".to_owned();
    let event = |kind: &str, inside: bool| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
            ("key".to_owned(), ConfigValue::String(key.clone())),
            ("inside".to_owned(), ConfigValue::Bool(inside)),
            ("x".to_owned(), ConfigValue::Integer(120)),
            ("y".to_owned(), ConfigValue::Integer(20)),
        ]))
    };
    let packet = |events: Vec<ConfigValue>, overflow: bool| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Integer(120)),
            ("y".to_owned(), ConfigValue::Integer(20)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(overflow)),
            ("over".to_owned(), ConfigValue::String(key.clone())),
            ("events".to_owned(), ConfigValue::Array(events)),
        ]))
    };
    readout.update(
        800,
        600,
        Some(packet(vec![event("down", true), event("up", false)], false)),
    );
    assert!(
        readout.controls().is_empty(),
        "an up outside does not select a speed"
    );
    readout.update(
        800,
        600,
        Some(packet(
            vec![
                event("down", true),
                event("cancel", false),
                event("up", true),
            ],
            false,
        )),
    );
    assert!(readout.controls().is_empty(), "cancel ends the press");
    readout.update(
        800,
        600,
        Some(packet(vec![event("down", true), event("up", true)], true)),
    );
    assert!(
        readout.controls().is_empty(),
        "overflow makes every event ineligible"
    );
}

#[test]
fn speed_box_hover_recolors_its_outline_and_press_fills_its_inner_bar() {
    let mut readout = Readout::new("");
    readout.deliver(&crate::readout_tests::snapshot(14, 1, None));
    let key = "speed_2".to_owned();
    let outlines: Vec<_> = readout
        .entries()
        .into_iter()
        .filter(|entry| entry.contains("ui/theme/speed_box.png") && entry.starts_with("create "))
        .collect();
    assert_eq!(outlines.len(), 5);
    let outline_id = outlines[1].split_whitespace().nth(1).unwrap();
    let inner = readout
        .entries()
        .into_iter()
        .find(|entry| entry.ends_with("124 20 4 14") && entry.contains("ui/theme/white.png"))
        .expect("second speed's inner bar at scale two");
    let inner_id = inner.split_whitespace().nth(1).unwrap();
    let event = |kind: &str| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
            ("key".to_owned(), ConfigValue::String(key.clone())),
            ("inside".to_owned(), ConfigValue::Bool(true)),
            ("x".to_owned(), ConfigValue::Integer(126)),
            ("y".to_owned(), ConfigValue::Integer(27)),
        ]))
    };
    let packet = |events: Vec<ConfigValue>, held: bool| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Integer(126)),
            ("y".to_owned(), ConfigValue::Integer(27)),
            ("held".to_owned(), ConfigValue::Bool(held)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            ("over".to_owned(), ConfigValue::String(key.clone())),
            ("events".to_owned(), ConfigValue::Array(events)),
        ]))
    };

    readout.update(800, 600, Some(packet(Vec::new(), false)));
    assert!(
        readout
            .tints()
            .contains(&format!("set_tint {outline_id} #a8261c")),
        "hover tints: {:?}",
        readout.tints()
    );
    readout.update(800, 600, Some(packet(vec![event("down")], true)));
    assert!(
        readout
            .tints()
            .contains(&format!("set_tint {inner_id} #a8261c"))
    );
}

#[test]
fn the_time_bar_date_has_no_transport_text_and_an_hour_change_replaces_only_its_glyph() {
    let formatted = Readout::script(
        r#"
local date = require("lib.time")
window.record(date.date({day = 9, month = 10, year = 768, hour = 14, speed = 3, paused = true}))
"#,
    );
    assert!(
        formatted
            .entries()
            .iter()
            .any(|entry| entry == "record 9 Oct 768 · 14:00")
    );

    let mut readout = Readout::new("");
    readout.deliver(&crate::readout_tests::snapshot(14, 3, None));
    let before = readout.entries().len();
    readout.deliver(&crate::readout_tests::snapshot(15, 3, None));
    let changed = &readout.entries()[before..];
    assert_eq!(changed.len(), 3, "one date glyph is replaced: {changed:?}");
    assert!(changed[0].starts_with("destroy "), "{changed:?}");
    assert!(
        changed[1] == "load ui/font/glyphs/U+0035.png",
        "{changed:?}"
    );
    assert!(
        changed[2].contains("ui/font/glyphs/U+0035.png"),
        "{changed:?}"
    );
}

#[test]
fn the_time_bar_theme_requires_both_speed_colours() {
    let readout = Readout::script(
        r##"
theme.kind = function() return {outline = "#000000", hot = "#ffffff"} end
local look = require("lib.theme")
local ok, message = pcall(look.kind, "time_bar")
window.record(tostring(ok) .. " " .. tostring(message))
theme.kind = function() return {outline = "#000000", hot = "#ffffff", speed_box = "#ece3cb"} end
local ok2, message2 = pcall(look.kind, "time_bar")
window.record(tostring(ok2) .. " " .. tostring(message2))
        "##,
    );
    let errors: Vec<_> = readout
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("record false "))
        .collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].contains("[kind.time_bar] missing `speed_box`"),
        "{errors:?}"
    );
    assert!(
        errors[1].contains("[kind.time_bar] missing `speed_fill`"),
        "{errors:?}"
    );
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
    let mut readout = Readout::new(crate::readout_tests::PLACE_TOOLTIP_TEST_FIXTURE);
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
    let before = panel_rects(&readout);
    readout.deliver_hover(Some(("town", "Alpha", 100, 160, 80)));
    assert!(
        panel_rects(&readout) == before,
        "the tooltip waits through its delay: before={before:?} after={:?}",
        panel_rects(&readout)
    );
    for _ in 0..20 {
        readout.update(800, 600, None);
    }
    let shown = panel_rects(&readout);
    assert_eq!(shown.len(), before.len() + 1, "{shown:?} vs {before:?}");
    assert!(
        on_screen(shown[0]),
        "the shown tooltip's panel is placed: {shown:?}"
    );
    readout.deliver_hover(None);
    readout.update(800, 600, None);
    let hidden = panel_rects(&readout);
    assert_eq!(
        hidden.len(),
        before.len(),
        "an unlocked tooltip is destroyed when its target leaves"
    );
    readout.deliver_hover(Some(("town", "Alpha", 100, 160, 80)));
    for _ in 0..20 {
        readout.update(800, 600, None);
    }
    let again = panel_rects(&readout);
    assert!(
        on_screen(again[0]),
        "and placed again when shown: {again:?}"
    );
}

#[test]
fn button_tooltips_open_change_on_the_next_hover_and_absent_labels_stay_hidden() {
    let mut readout = Readout::new(
        r#"
local button = require("lib.button")
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local text = require("lib.text")
local real_set = text.set
local capture = false
text.set = function(line, value)
  real_set(line, value)
  if capture then window.record("tooltip_text " .. value) end
end
local labelled = button.new{x = 10, y = 20, text = "Go", tooltip = "yield map"}
local plain = button.new{x = 100, y = 20, text = "No"}
local updates = 0
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
  updates = updates + 1
  if updates == 24 then labelled:set_tooltip("play") end
end)
window.record("labelled " .. labelled.key)
window.record("plain " .. plain.key)
capture = true
"#,
    );
    let key = |name: &str| {
        records(&readout)
            .iter()
            .find_map(|entry| entry.strip_prefix(&format!("{name} ")).map(str::to_owned))
            .expect("the button key")
    };
    let labelled = key("labelled");
    let plain = key("plain");
    let hover = |key: &str, x| pointer_packet(key, x, vec![pointer_event("move", Some(key), None)]);
    readout.update(800, 600, Some(hover(&labelled, 20.0)));
    for _ in 0..24 {
        readout.update(800, 600, Some(hover(&labelled, 20.0)));
    }
    let after_first = records(&readout);
    assert!(
        after_first
            .iter()
            .any(|entry| entry == "tooltip_text yield map")
    );

    readout.update(800, 600, Some(pointer_packet("", 0.0, Vec::new())));
    let before_changed = records(&readout).len();
    for _ in 0..22 {
        readout.update(800, 600, Some(hover(&labelled, 20.0)));
    }
    let changed = records(&readout);
    assert!(
        changed[before_changed..]
            .iter()
            .any(|entry| entry == "tooltip_text play"),
        "set_tooltip did not change the next tooltip: {changed:?}"
    );

    readout.update(800, 600, Some(pointer_packet("", 0.0, Vec::new())));
    let before_plain = records(&readout).len();
    for _ in 0..22 {
        readout.update(800, 600, Some(hover(&plain, 110.0)));
    }
    let final_records = records(&readout);
    assert!(
        !final_records[before_plain..]
            .iter()
            .any(|entry| entry.starts_with("tooltip_text ")),
        "a button without a tooltip opened one: {final_records:?}"
    );
}

#[test]
fn attached_tooltip_waits_locks_colours_and_survives_leaving_its_target() {
    let mut readout = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local key = "tooltip_target"
local widget = {on_event = function() end, on_hover = function() end, on_overflow = function() end}
pointer.register(key, widget)
render.set_hit(key, {{x = 20, y = 20, width = 80, height = 24}})
tooltip.define("population", function()
  return {title = "Population", {"value ", {text = "96.5", class = "population", args = {}}}}
end)
tooltip.attach(key, {class = "population", args = {}})
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
end)
"#,
        TooltipsConfig::default(),
    );
    let pointer = |over: Option<&str>| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Float(32.0)),
            ("y".to_owned(), ConfigValue::Float(28.0)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            (
                "over".to_owned(),
                over.map_or(ConfigValue::Bool(false), |key| {
                    ConfigValue::String(key.to_owned())
                }),
            ),
            ("events".to_owned(), ConfigValue::Array(Vec::new())),
        ]))
    };
    readout.update(800, 600, Some(pointer(Some("tooltip_target"))));
    assert!(
        !readout.entries().iter().any(|entry| {
            entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
        }),
        "the configured 0.3 second delay has not elapsed"
    );
    for _ in 0..17 {
        readout.update(800, 600, Some(pointer(Some("tooltip_target"))));
    }
    assert!(
        !readout.entries().iter().any(|entry| {
            entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
        }),
        "0.288 seconds including the first frame's 0.016 seconds is still below the delay"
    );
    readout.update(800, 600, Some(pointer(Some("tooltip_target"))));
    let shown = readout.entries();
    assert!(
        shown.iter().any(|entry| {
            entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
        }),
        "tooltip opens after the delay: {shown:?}"
    );
    let tints = readout.tints();
    assert!(
        tints.iter().any(|entry| entry.ends_with("#c9932e")),
        "{tints:?}"
    );
    assert!(
        tints.iter().any(|entry| entry.ends_with("#5fb35a")),
        "{tints:?}"
    );
    for _ in 0..64 {
        readout.update(800, 600, Some(pointer(Some("tooltip_target"))));
    }
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry.starts_with("set_hit tooltip_panel_")),
        "timer lock publishes a hit region"
    );
    let before_leave = readout.entries().len();
    readout.update(800, 600, Some(pointer(None)));
    assert!(
        !readout.entries()[before_leave..]
            .iter()
            .any(|entry| entry.starts_with("destroy q")),
        "a locked tooltip remains after the pointer leaves"
    );
}

#[test]
fn a_locked_link_survives_live_refresh_until_its_child_opens_and_locks() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local active = true
pointer.register("live_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("live_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.define("A", function() return active and {{{text = "B", class = "B", args = {}}}} or nil end)
tooltip.define("B", function() return {title = "B", "child body"} end)
tooltip.attach("live_target", {class = "A", args = {}})
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.refresh()
  tooltip.update(update)
  if update.pointer.over == "nil_refresh" then active = false; tooltip.refresh() end
end)
"#,
    );
    let pointer = |over: &str| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Float(32.0)),
            ("y".to_owned(), ConfigValue::Float(28.0)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            ("over".to_owned(), ConfigValue::String(over.to_owned())),
            ("events".to_owned(), ConfigValue::Array(Vec::new())),
        ]))
    };
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer("live_target")));
    }
    let span_key = readout
        .entries()
        .iter()
        .rev()
        .find_map(|entry| {
            entry
                .strip_prefix("set_hit ")
                .and_then(|rest| rest.split_once(' '))
                .map(|(key, _)| key.to_owned())
                .filter(|key| key.starts_with("tooltip_span_"))
        })
        .expect("the locked parent exposes its link");
    for _ in 0..20 {
        readout.update(800, 600, Some(pointer(&span_key)));
    }
    let opened = readout.entries();
    assert!(
        opened
            .iter()
            .filter(
                |entry| entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
            )
            .count()
            >= 2,
        "live refresh must not restart the child's delay: {opened:?}"
    );
    for _ in 0..64 {
        readout.update(800, 600, Some(pointer(&span_key)));
    }
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry.starts_with("set_hit tooltip_panel_2 ")),
        "the child must reach its timer lock while live refresh continues"
    );
    readout.update(800, 600, Some(pointer("nil_refresh")));
    assert!(
        current_hits(&readout.entries(), "tooltip_panel_").is_empty(),
        "nil live content closes the parent and its child"
    );
}

#[test]
fn an_explanation_wraps_at_160_font_pixels() {
    let mut readout = Readout::script_with_glossary(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("term_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("term_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("term_target", function() return {{{text = "term", term = "sample"}}} end)
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
end)
"#,
        "[term.sample]\ntitle = \"Sample\"\ntext = \"12345678 12345678 12345678 12345678\"\n",
    );
    let pointer = |over: &str| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Float(32.0)),
            ("y".to_owned(), ConfigValue::Float(28.0)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            ("over".to_owned(), ConfigValue::String(over.to_owned())),
            ("events".to_owned(), ConfigValue::Array(Vec::new())),
        ]))
    };
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer("term_target")));
    }
    let span_key = readout
        .entries()
        .iter()
        .rev()
        .find_map(|entry| {
            entry
                .strip_prefix("set_hit ")
                .and_then(|rest| rest.split_once(' '))
                .map(|(key, _)| key.to_owned())
                .filter(|key| key.starts_with("tooltip_span_"))
        })
        .expect("the locked term is a link");
    let start = readout.entries().len();
    for _ in 0..20 {
        readout.update(800, 600, Some(pointer(&span_key)));
    }
    let child_glyph_rows: std::collections::BTreeSet<_> = readout.entries()[start..]
        .iter()
        .filter(|entry| entry.starts_with("create ") && entry.contains("ui/font/small/"))
        .filter_map(|entry| entry.split_whitespace().nth(4)?.parse::<i64>().ok())
        .collect();
    let mut glyph_counts: Vec<usize> = readout.entries()[start..]
        .iter()
        .filter(|entry| entry.starts_with("create ") && entry.contains("ui/font/small/"))
        .fold(BTreeMap::<i64, usize>::new(), |mut rows, entry| {
            let y = entry.split_whitespace().nth(4).unwrap().parse().unwrap();
            *rows.entry(y).or_default() += 1;
            rows
        })
        .into_values()
        .collect();
    glyph_counts.sort_unstable();
    assert_eq!(
        child_glyph_rows.len(),
        3,
        "the tooltip title and two wrapped explanation rows: {child_glyph_rows:?}"
    );
    assert_eq!(
        glyph_counts,
        [6, 8, 24],
        "three 40-pixel words fit on the first row; the next word exceeds 160 pixels"
    );
}

#[test]
fn a_full_lock_bar_reaches_the_last_inside_pixel_of_the_cut_corner() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("bar_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("bar_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("bar_target", function() return {"bar"} end)
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
end)
"#,
    );
    let pointer = ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Float(32.0)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("held".to_owned(), ConfigValue::Bool(false)),
        ("overflow".to_owned(), ConfigValue::Bool(false)),
        (
            "over".to_owned(),
            ConfigValue::String("bar_target".to_owned()),
        ),
        ("events".to_owned(), ConfigValue::Array(Vec::new())),
    ]));
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer.clone()));
    }
    let tints = readout.tints();
    let bar = tints
        .iter()
        .find_map(|entry| {
            entry
                .strip_prefix("create ")
                .and_then(|rest| rest.split_once(' '))
                .filter(|(_, tint)| *tint == "#f0a830")
                .map(|(quad, _)| quad.to_owned())
        })
        .expect("the full timer bar uses the lock colour");
    let rect = quads(&readout.entries())[&bar].to_owned();
    let entries = readout.entries();
    let panel = entries
        .iter()
        .find(|entry| entry.starts_with("set_hit tooltip_panel_1 "))
        .expect("the locked panel rectangle");
    let panel_values: Vec<i64> = panel
        .split_whitespace()
        .skip(2)
        .map(|value| value.parse().expect("panel coordinate"))
        .collect();
    let p = 2;
    assert_eq!(rect.0, panel_values[0] + p);
    assert_eq!(rect.1, panel_values[1] + panel_values[3] - 2 * p);
    assert_eq!(rect.2, panel_values[2] - 4 * p);
    assert_eq!(rect.3, p);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one delay scenario checks target switching, part switching and nil sources against the same frame driver"
)]
fn tooltip_delay_restarts_on_key_and_part_changes_and_nil_content_stays_hidden() {
    let mut readout = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local widget = {on_event = function() end, on_hover = function() end, on_overflow = function() end}
for _, key in ipairs({"a", "b", "nil_source", "part_source"}) do
  pointer.register(key, widget)
  render.set_hit(key, {{x = 20, y = 20, width = 80, height = 24}})
end
tooltip.attach("a", function() return {"A"} end)
tooltip.attach("b", function() return {"B"} end)
tooltip.attach("nil_source", function() return nil end)
tooltip.attach("part_source", function(x) return {"part"}, x < 100 and "left" or "right" end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
        TooltipsConfig {
            delay: 0.3,
            lock: "disabled",
            lock_timer: 1.0,
        },
    );
    for _ in 0..12 {
        readout.update(800, 600, Some(pointer_packet("a", 32.0, Vec::new())));
    }
    for _ in 0..12 {
        readout.update(800, 600, Some(pointer_packet("b", 32.0, Vec::new())));
    }
    assert_eq!(
        readout
            .entries()
            .iter()
            .filter(|entry| entry.contains("ui/theme/cut3_tl_fill.png"))
            .count(),
        0,
        "switching at 0.2 seconds restarts the delay"
    );
    for _ in 0..7 {
        readout.update(800, 600, Some(pointer_packet("b", 32.0, Vec::new())));
    }
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry.contains("ui/theme/cut3_tl_fill.png"))
    );
    let before_nil = readout.entries().len();
    for _ in 0..24 {
        readout.update(
            800,
            600,
            Some(pointer_packet("nil_source", 32.0, Vec::new())),
        );
    }
    assert!(
        !readout.entries()[before_nil..].iter().any(
            |entry| entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
        ),
        "a nil source opens nothing"
    );

    let mut part = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("part_source", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("part_source", {{x = 20, y = 20, width = 180, height = 24}})
tooltip.attach("part_source", function(x) return {"part"}, x < 100 and "left" or "right" end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
        TooltipsConfig {
            delay: 0.3,
            lock: "disabled",
            lock_timer: 1.0,
        },
    );
    for _ in 0..12 {
        part.update(
            800,
            600,
            Some(pointer_packet("part_source", 32.0, Vec::new())),
        );
    }
    for _ in 0..12 {
        part.update(
            800,
            600,
            Some(pointer_packet("part_source", 120.0, Vec::new())),
        );
    }
    assert!(
        !part
            .entries()
            .iter()
            .any(|entry| entry.contains("ui/theme/cut3_tl_fill.png")),
        "changing part restarts the delay"
    );
    for _ in 0..7 {
        part.update(
            800,
            600,
            Some(pointer_packet("part_source", 120.0, Vec::new())),
        );
    }
    assert!(
        part.entries()
            .iter()
            .any(|entry| entry.contains("ui/theme/cut3_tl_fill.png"))
    );
}

#[test]
fn shared_tooltip_definitions_and_glossary_lua_extensions_return_their_contract_values() {
    let mut readout = Readout::script_with_glossary(
        r#"
local tooltip = require("lib.tooltip")
local definition = function(args) return {"population " .. args.place} end
tooltip.define("population", definition)
assert(tooltip.content("population", {place = "p1"})[1] == "population p1")
local duplicate, duplicate_error = pcall(tooltip.define, "population", definition)
assert(not duplicate and duplicate_error:find("population", 1, true))
local missing, missing_error = pcall(tooltip.content, "missing", {})
assert(not missing and missing_error:find("missing", 1, true))
local entry = glossary.entry("population")
assert(entry.key == "population" and entry.title == "Population" and entry.text == "See {pop|pops}")
assert(table.concat(glossary.keys(), ",") == "pop,population")
frame.on_update(function() end)
"#,
        "[term.pop]\ntitle = \"Pop\"\ntext = \"People\"\n[term.population]\ntitle = \"Population\"\ntext = \"See {pop|pops}\"\n",
    );
    readout.update(800, 600, None);
    assert!(
        readout.entries().is_empty(),
        "the Lua contract assertions ran without drawing UI"
    );
}

const LOCATION_TOOLTIP_CLASSES: &str = r#"
local classes = require("lib.classes")
local function pop(kind, culture, religion, workers, dependents)
  return {kind = kind, culture = culture, religion = religion, workers = workers, dependents = dependents}
end
local goods = {"oats", "barley", "peas_beans", "vegetables", "apples", "dairy", "beef", "pork", "mutton", "fish", "seabirds", "ale", "mead"}
local produced, used = {}, {}
for index, good in ipairs(goods) do
  produced[index] = {good = good, units = index,
    value_milli = index == 1 and 1500000 or index == 2 and 1500000 or index == 3 and 1209500 or 0}
  used[index] = {good = good, units = index, value_milli = index == 1 and 2000000 or index == 2 and 1980000 or 0}
end
local fixture = {
  id = "clonmacnoise", workers = 272, dependents = 140,
  pops = {pop("peasant", "mumu", "christianity", 200, 100), pop("labourer", "laigin", "christianity", 72, 40)},
  land = {usable = {pasture = 32, forest = 12, scrub = 6},
    land = {pasture = 32, forest = 12, scrub = 6, bog = 16, mountain = 10}, sea = 8},
  last_month = {closed = true, produced = produced, used_by_pops = used, lost = {}},
}
local population = {
  id = "kinds", workers = 188, dependents = 224,
  pops = {pop("monk", "mumu", "christianity", 20, 20), pop("labourer", "mumu", "christianity", 60, 0),
    pop("peasant", "mumu", "christianity", 200, 112)},
  land = fixture.land, last_month = fixture.last_month,
}
local ties = {
  id = "ties", workers = 206, dependents = 206,
  pops = {pop("peasant", "mumu", "christianity", 103, 103), pop("labourer", "laigin", "christianity", 103, 103)},
  land = fixture.land, last_month = fixture.last_month,
}
local equal_kinds = {
  id = "equal_kinds", workers = 20, dependents = 0,
  pops = {pop("monk", "mumu", "christianity", 10, 0), pop("labourer", "mumu", "christianity", 10, 0)},
  land = fixture.land, last_month = fixture.last_month,
}
local empty = {id = "empty", workers = 0, dependents = 0, pops = {}, land = {usable = {}, land = {}, sea = 0},
  last_month = {closed = false, produced = {}, used_by_pops = {}, lost = {}}}
local closed_empty = {id = "closed_empty", workers = 0, dependents = 0, pops = {}, land = empty.land,
  last_month = {closed = true, produced = {}, used_by_pops = {}, lost = {}}}
game.places = function() return {
  {id = "clonmacnoise", name = "Cluain Mhic Nóis"}, {id = "kinds", name = "Kinds"},
  {id = "ties", name = "Ties"}, {id = "equal_kinds", name = "Equal Kinds"},
  {id = "empty", name = "Empty"}, {id = "closed_empty", name = "Closed Empty"},
} end
game.place = function(id, revision)
  assert(revision == 7)
  return ({clonmacnoise = fixture, kinds = population, ties = ties, equal_kinds = equal_kinds,
    empty = empty, closed_empty = closed_empty})[id]
end
classes.set_revision(7)
local tooltip = require("lib.tooltip")
local function line_text(line)
  local chunks = {}
  for _, span in ipairs(line) do chunks[#chunks + 1] = type(span) == "string" and span or span.text end
  return table.concat(chunks)
end
local function texts(content)
  local out = {}
  for index, line in ipairs(content) do out[index] = line_text(line) end
  return out
end
local function has_span(line, wanted, field, value)
  for _, span in ipairs(line) do
    if type(span) == "table" and span.text == wanted and span[field] == value then return true end
  end
  return false
end
local place = tooltip.content("location", {place = "clonmacnoise"})
assert(place.title == "Cluain Mhic Nóis")
local place_lines = texts(place)
assert(table.concat(place_lines, "\n") == table.concat({
  "population 412", "culture Mumu 72%", "religion Christianity 100%", "usable land 12.5 km²",
  "produced last month 4210", "consumed last month 3980",
}, "\n"), table.concat(place_lines, "\n"))
assert(has_span(place[1], "412", "class", "population"))
assert(place[1][3].args.place == "clonmacnoise")
assert(has_span(place[2], "Mumu", "term", "mumu"))
assert(has_span(place[2], "72%", "class", "cultures"))
assert(has_span(place[3], "100%", "class", "religions"))
assert(has_span(place[4], "12.5", "class", "land"))
assert(has_span(place[5], "4210", "class", "produced"))
assert(has_span(place[6], "3980", "class", "consumed"))
assert(not table.concat(place_lines, " "):find("county", 1, true))
assert(not table.concat(place_lines, " "):find("region", 1, true))

local populations = texts(tooltip.content("population", {place = "kinds"}))
assert(table.concat(populations, "\n") == "peasants 312\nlabourers 60\nmonks 40\nworkers 188, dependents 224")
assert(table.concat(texts(tooltip.content("population", {place = "equal_kinds"})), "\n") ==
  "labourers 10\nmonks 10\nworkers 20, dependents 0")
local tied_place = tooltip.content("location", {place = "ties"})
assert(tied_place.title == "Ties" and line_text(tied_place[2]) == "culture Laigin 50%")
assert(has_span(tied_place[2], "Laigin", "term", "laigin"))
assert(table.concat(texts(tooltip.content("cultures", {place = "ties"})), "\n") ==
  "Laigin 206 (50%)\nMumu 206 (50%)")
local culture_lines = texts(tooltip.content("cultures", {place = "clonmacnoise"}))
assert(table.concat(culture_lines, "\n") == "Mumu 300 (72%)\nLaigin 112 (27%)")
local religion_lines = texts(tooltip.content("religions", {place = "clonmacnoise"}))
assert(table.concat(religion_lines, "\n") == "Christianity 412 (100%)")
local land_lines = texts(tooltip.content("land", {place = "clonmacnoise"}))
assert(table.concat(land_lines, "\n") == "pasture 8\nforest 3\nscrub 1.5\nnot usable 6.5\nsea 2")
local made = tooltip.content("produced", {place = "clonmacnoise"})
assert(#made == 14 and line_text(made[1]) == "value at local prices:")
assert(line_text(made[2]) == "oats 1500" and line_text(made[3]) == "barley 1500")
assert(line_text(made[4]) == "peas and beans 1210")
assert(line_text(made[14]) == "mead 0")
local eaten = texts(tooltip.content("consumed", {place = "clonmacnoise"}))
assert(eaten[1] == "value at local prices:" and eaten[2] == "oats 2000" and eaten[3] == "barley 1980" and eaten[14] == "mead 0")
local empty_lines = texts(tooltip.content("location", {place = "empty"}))
assert(table.concat(empty_lines, "\n") == "population 0\nusable land 0 km²\nproduced last month -\nconsumed last month -")
local open_place = tooltip.content("location", {place = "empty"})
assert(not has_span(open_place[3], "-", "class", "produced") and not has_span(open_place[4], "-", "class", "consumed"))
local closed_lines = tooltip.content("location", {place = "closed_empty"})
assert(has_span(closed_lines[3], "0", "class", "produced"))
assert(has_span(closed_lines[4], "0", "class", "consumed"))
assert(tooltip.content("location", {place = "not-yet-in-view"}) == nil)
frame.on_update(function() end)
"#;

#[test]
fn location_tooltip_classes_cover_the_economy_breakdowns_and_ordering_contract() {
    let mut readout = Readout::script(LOCATION_TOOLTIP_CLASSES);
    readout.update(800, 600, None);
    assert!(
        readout.entries().is_empty(),
        "the class assertions execute without drawing UI"
    );
}

#[test]
fn empty_tooltip_content_is_nil_on_open_and_live_refresh() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local empty = false
pointer.register("empty_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("empty_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("empty_target", function() return empty and {} or {"visible"} end)
frame.on_update(function(update)
  tooltip.update(update)
  if update.pointer.over == "refresh_empty" then empty = true; tooltip.refresh() end
end)
"#,
    );
    for _ in 0..20 {
        readout.update(
            800,
            600,
            Some(pointer_packet("empty_target", 32.0, Vec::new())),
        );
    }
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry.contains("ui/theme/cut3_tl_fill.png"))
    );
    let before = readout.entries().len();
    readout.update(
        800,
        600,
        Some(pointer_packet("refresh_empty", 32.0, Vec::new())),
    );
    assert!(
        readout.entries()[before..]
            .iter()
            .any(|entry| entry.starts_with("destroy q")),
        "an empty refresh closes the old panel"
    );
    let before_empty = readout.entries().len();
    for _ in 0..20 {
        readout.update(
            800,
            600,
            Some(pointer_packet("empty_target", 32.0, Vec::new())),
        );
    }
    assert!(
        !readout.entries()[before_empty..].iter().any(
            |entry| entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
        ),
        "empty content behaves like nil"
    );
}

#[test]
fn tooltip_content_without_a_measurable_line_is_nil_on_open_and_refresh() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local empty_line = false
pointer.register("empty_line_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("empty_line_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("empty_line_target", function() return empty_line and {{}} or {"visible"} end)
frame.on_update(function(update)
  tooltip.update(update)
  if update.pointer.over == "refresh_empty_line" then empty_line = true; tooltip.refresh() end
end)
"#,
    );
    for _ in 0..20 {
        readout.update(
            800,
            600,
            Some(pointer_packet("empty_line_target", 32.0, Vec::new())),
        );
    }
    assert_eq!(tooltip_panel_count(&readout), 1);
    let before_refresh = readout.entries().len();
    readout.update(
        800,
        600,
        Some(pointer_packet("refresh_empty_line", 32.0, Vec::new())),
    );
    assert!(
        readout.entries()[before_refresh..]
            .iter()
            .any(|entry| entry.starts_with("destroy q")),
        "refreshing to a line with no glyphs closes the former panel"
    );
    let before_open = readout.entries().len();
    for _ in 0..20 {
        readout.update(
            800,
            600,
            Some(pointer_packet("empty_line_target", 32.0, Vec::new())),
        );
    }
    assert!(
        !readout.entries()[before_open..].iter().any(|entry| {
            entry.starts_with("create ") && entry.contains("ui/theme/cut3_tl_fill.png")
        }),
        "{{}} has no measurable line and creates no 9-by-9 frame"
    );
}

#[test]
fn tooltip_look_rows_pin_oats_height_width_and_both_numeric_tints() {
    let mut readout = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local content = {"oats", {"price ", {text = "96.5", tooltip = function() return {"price detail"} end}, ", base 100"}}
pointer.register("oats_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("oats_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("oats_target", function() return content end)
local width = 6 + math.max(font.layout("oats", "small").width,
  font.layout("price ", "small").width + font.layout("96.5", "small").width + font.layout(", base 100", "small").width)
window.record("oats expected " .. width)
frame.on_update(function(update) tooltip.update(update) end)
"#,
        TooltipsConfig {
            delay: 0.0,
            lock: "disabled",
            lock_timer: 1.0,
        },
    );
    readout.update(
        800,
        600,
        Some(pointer_packet("oats_target", 32.0, Vec::new())),
    );
    readout.update(
        800,
        600,
        Some(pointer_packet("oats_target", 32.0, Vec::new())),
    );
    let rect = tooltip_panel_rect(&readout).expect("the oats panel");
    let expected_width = records(&readout)
        .iter()
        .find_map(|record| record.strip_prefix("oats expected "))
        .expect("fixture width")
        .parse::<i64>()
        .expect("integer width");
    assert_eq!(
        rect.2,
        expected_width * 2,
        "panel is six pixels wider than its longest line"
    );
    assert_eq!(rect.3, 44, "two small-font rows are 22 pixels high");
    assert_tooltip_text_tint(&readout, "oats", "#ece3cb");
    assert_tooltip_text_tint(&readout, "price", "#ece3cb");
    assert_tooltip_text_tint(&readout, "96.5", "#5fb35a");
    assert_tooltip_text_tint(&readout, "base", "#ece3cb");
    assert_tooltip_text_tint(&readout, "100", "#5fb35a");
}

#[test]
fn tooltip_colour_cases_pin_plain_numbers_link_colours_and_locked_titles() {
    let mut readout = Readout::script_with_glossary_and_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local function detail() return {"detail"} end
pointer.register("colour_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("colour_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("colour_target", function() return {
  title = "Cluain Mhic Nóis",
  {{text = "population", term = "population"}, " ", {text = "412", class = "population", args = {place = "p1"}}},
  "4,210 73% 12.5 km²",
  {{text = "more", tooltip = detail}}
} end)
tooltip.define("population", function(args) return {"population " .. args.place} end)
frame.on_update(function(update) pointer.dispatch(update.pointer); tooltip.update(update) end)
"#,
        "[term.population]\ntitle = \"Population\"\ntext = \"People\"\n",
        TooltipsConfig {
            delay: 0.3,
            lock: "middle_mouse",
            lock_timer: 1.0,
        },
    );
    for _ in 0..20 {
        readout.update(
            800,
            600,
            Some(pointer_packet("colour_target", 32.0, Vec::new())),
        );
    }
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "colour_target",
            32.0,
            vec![ConfigValue::Table(BTreeMap::from([
                (
                    "kind".to_owned(),
                    ConfigValue::String("button_down".to_owned()),
                ),
                (
                    "button".to_owned(),
                    ConfigValue::String("middle".to_owned()),
                ),
                ("key".to_owned(), ConfigValue::Bool(false)),
                ("x".to_owned(), ConfigValue::Float(32.0)),
                ("y".to_owned(), ConfigValue::Float(28.0)),
                ("inside".to_owned(), ConfigValue::Bool(false)),
            ]))],
        )),
    );
    assert_tooltip_text_tint(&readout, "Cluain", "#c9932e");
    assert_tooltip_text_tint(&readout, "population", "#92ddea");
    assert_tooltip_text_tint(&readout, "412", "#5fb35a");
    assert_tooltip_text_tint(&readout, "4,210", "#5fb35a");
    assert_tooltip_text_tint(&readout, "73%", "#5fb35a");
    assert_tooltip_text_tint(&readout, "12.5", "#5fb35a");
    assert_tooltip_text_tint(&readout, "km", "#ece3cb");
    assert_tooltip_text_tint(&readout, "more", "#ece3cb");
    readout.update(
        800,
        600,
        Some(pointer_packet("tooltip_panel_1", 38.0, Vec::new())),
    );
    assert_eq!(
        tooltip_panel_count(&readout),
        1,
        "hovering a locked title opens no child"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one literal scroll contract follows the same 20-line tooltip through every required row and refresh case"
)]
fn long_tooltip_scroll_coverage_pins_thumb_steps_clamps_mixed_rows_and_refresh() {
    let mut readout = tooltip_scroll_readout("middle_mouse", true);
    readout.update(
        800,
        600,
        Some(pointer_packet("long_target", 32.0, Vec::new())),
    );
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "long_target",
            32.0,
            vec![ConfigValue::Table(BTreeMap::from([
                (
                    "kind".to_owned(),
                    ConfigValue::String("button_down".to_owned()),
                ),
                (
                    "button".to_owned(),
                    ConfigValue::String("middle".to_owned()),
                ),
                ("key".to_owned(), ConfigValue::Bool(false)),
                ("x".to_owned(), ConfigValue::Float(32.0)),
                ("y".to_owned(), ConfigValue::Float(28.0)),
                ("inside".to_owned(), ConfigValue::Bool(false)),
            ]))],
        )),
    );
    let panel_y = current_hits(&readout.entries(), "tooltip_panel_")["tooltip_panel_1"]
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let initial_thumb = tooltip_scroll_thumb(&readout);
    assert_eq!(
        initial_thumb.1,
        panel_y + 6,
        "105×12/20 gives a 63-unit thumb at the top"
    );
    assert_eq!(initial_thumb.3, 126);
    assert_eq!(
        tooltip_text_rects(&readout, "row13").len(),
        0,
        "line 13 is initially hidden"
    );
    assert_mixed_tooltip_rows(&readout, 1, panel_y);

    readout.update(
        800,
        600,
        Some(pointer_packet(
            "long_target",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(-3.0),
                Some("lines"),
            )],
        )),
    );
    let scrolled_thumb = tooltip_scroll_thumb(&readout);
    assert_eq!(
        scrolled_thumb.1,
        panel_y + 38,
        "scrolling to first=4 moves the thumb to the rounded 38-unit offset"
    );
    let first_four = current_hits(&readout.entries(), "tooltip_span_");
    assert!(
        first_four.keys().any(|key| key.contains("_4:1"))
            && !first_four.keys().any(|key| key.contains("_1:1"))
    );
    assert_mixed_tooltip_rows(&readout, 4, panel_y);
    assert_eq!(
        tooltip_text_rects(&readout, "row16").len(),
        0,
        "line 16 remains below the viewport"
    );
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "tooltip_panel_1",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(3.0),
                Some("lines"),
            )],
        )),
    );
    assert_eq!(
        tooltip_scroll_thumb(&readout).1,
        panel_y + 6,
        "+3 returns the thumb and rows to the top"
    );
    assert_mixed_tooltip_rows(&readout, 1, panel_y);
    let at_top = current_hits(&readout.entries(), "tooltip_span_");
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "tooltip_panel_1",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(1.0),
                Some("lines"),
            )],
        )),
    );
    assert_eq!(
        current_hits(&readout.entries(), "tooltip_span_"),
        at_top,
        "+1 at first=1 is clamped"
    );
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "tooltip_panel_1",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(-1.5),
                Some("lines"),
            )],
        )),
    );
    assert_mixed_tooltip_rows(&readout, 2, panel_y);
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "tooltip_panel_1",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(-1.5),
                Some("lines"),
            )],
        )),
    );
    assert_mixed_tooltip_rows(&readout, 4, panel_y);
    let first_four = current_hits(&readout.entries(), "tooltip_span_");
    assert!(
        first_four.keys().any(|key| key.contains("_4:1"))
            && !first_four.keys().any(|key| key.contains("_3:1")),
        "two -1.5-line scrolls advance from first=1 to first=4"
    );
    readout.update(
        800,
        600,
        Some(pointer_packet(
            "tooltip_panel_1",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(-30.0),
                Some("lines"),
            )],
        )),
    );
    assert!(
        current_hits(&readout.entries(), "tooltip_span_")
            .keys()
            .any(|key| key.contains("_9:1")),
        "scroll clamps at first=9"
    );
    readout.update(
        800,
        600,
        Some(pointer_packet("refresh_short", 32.0, Vec::new())),
    );
    let refreshed = current_hits(&readout.entries(), "tooltip_span_");
    assert!(
        refreshed.keys().any(|key| key.contains("_4:1"))
            && !refreshed.keys().any(|key| key.contains("_3:1")),
        "refreshing to 15 lines clamps first=9 to 4"
    );

    let mut unlocked = tooltip_scroll_readout("disabled", false);
    unlocked.update(
        800,
        600,
        Some(pointer_packet("long_target", 32.0, Vec::new())),
    );
    unlocked.update(
        800,
        600,
        Some(pointer_packet("long_target", 32.0, Vec::new())),
    );
    let row_one_before = tooltip_text_rects(&unlocked, "row01");
    let row_four_before = tooltip_text_rects(&unlocked, "row04");
    unlocked.update(
        800,
        600,
        Some(pointer_packet(
            "long_target",
            32.0,
            vec![long_tooltip_pointer_event(
                "scroll",
                "tooltip_panel_1",
                Some(-3.0),
                Some("lines"),
            )],
        )),
    );
    assert!(
        tooltip_text_rects(&unlocked, "row01") == row_one_before
            && tooltip_text_rects(&unlocked, "row04") == row_four_before,
        "an unlocked tooltip ignores scroll: row01={:?}, row04={:?}, panels={}",
        tooltip_text_rects(&unlocked, "row01"),
        tooltip_text_rects(&unlocked, "row04"),
        tooltip_panel_count(&unlocked)
    );
}

#[test]
fn live_refresh_redraws_a_changed_value_in_place() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local value = "96.5"
pointer.register("live_value", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("live_value", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.define("price", function() window.record("rendered price " .. value); return {{"price ", {text = value, tooltip = function() return {"detail"} end}}} end)
tooltip.attach("live_value", {class = "price", args = {}})
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  if update.pointer.over == "refresh_value" then value = "97"; tooltip.refresh() end
  tooltip.update(update)
end)
"#,
    );
    for _ in 0..82 {
        readout.update(
            800,
            600,
            Some(pointer_packet("live_value", 32.0, Vec::new())),
        );
    }
    let before_hits = current_hits(&readout.entries(), "tooltip_span_");
    readout.update(
        800,
        600,
        Some(pointer_packet("refresh_value", 32.0, Vec::new())),
    );
    let after_hits = current_hits(&readout.entries(), "tooltip_span_");
    assert_eq!(
        before_hits.keys().collect::<Vec<_>>(),
        after_hits.keys().collect::<Vec<_>>()
    );
    assert!(
        records(&readout)
            .iter()
            .any(|record| record == "rendered price 97")
    );
}

#[test]
fn a_glossary_term_inside_an_explanation_is_blue_and_opens_its_own_explanation() {
    let mut readout = Readout::script_with_glossary(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("term_root", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("term_root", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("term_root", function() return {{{text = "pops", term = "pop"}}} end)
frame.on_update(function(update) pointer.dispatch(update.pointer); tooltip.update(update) end)
"#,
        "[term.pop]\ntitle = \"Population\"\ntext = \"See {county}\"\n[term.county]\ntitle = \"County\"\ntext = \"Land\"\n",
    );
    for _ in 0..82 {
        readout.update(
            800,
            600,
            Some(pointer_packet("term_root", 32.0, Vec::new())),
        );
    }
    let pop = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_1"))
        .unwrap()
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&pop, 52.0, Vec::new())));
    }
    assert!(
        readout
            .tints()
            .iter()
            .any(|entry| entry.ends_with("#92ddea")),
        "term spans use the theme's blue"
    );
    let county = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_2"))
        .expect("the explanation contains a linked county term")
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&county, 72.0, Vec::new())));
    }
    assert!(current_hits(&readout.entries(), "tooltip_panel_").contains_key("tooltip_panel_3"));
}

#[test]
fn unrelated_timer_locks_can_coexist_and_the_newest_closes_first() {
    let mut readout = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
for _, key in ipairs({"first", "second"}) do
  pointer.register(key, {on_event = function() end, on_hover = function() end, on_overflow = function() end})
  render.set_hit(key, {{x = 20, y = 20, width = 80, height = 24}})
  tooltip.attach(key, function() return {key} end)
end
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
  if update.pointer.over == "close_top" then window.record("closed " .. tostring(tooltip.close_top())) end
end)
"#,
        TooltipsConfig {
            delay: 0.3,
            lock: "timer",
            lock_timer: 1.0,
        },
    );
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("first", 32.0, Vec::new())));
    }
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("second", 32.0, Vec::new())));
    }
    assert_eq!(current_hits(&readout.entries(), "tooltip_panel_").len(), 2);
    readout.update(
        800,
        600,
        Some(pointer_packet("close_top", 400.0, Vec::new())),
    );
    assert_eq!(current_hits(&readout.entries(), "tooltip_panel_").len(), 1);
    assert!(current_hits(&readout.entries(), "tooltip_panel_").contains_key("tooltip_panel_1"));
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry == "record closed true")
    );
}

#[test]
fn twelve_rows_have_no_scrollbar_and_mixed_fonts_keep_the_tallest_line_height() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("mixed", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("mixed", {{x = 20, y = 20, width = 80, height = 24}})
local lines = {{"small", {text = "tiny", font = "tiny"}}}
for index = 2, 12 do lines[index] = "row " .. index end
tooltip.attach("mixed", function() return lines end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
    );
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("mixed", 32.0, Vec::new())));
    }
    let entries = readout.entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry.starts_with("set_hit tooltip_panel_1 ") && entry.ends_with(" 224")),
        "twelve 9-pixel rows plus four pixels produce a 112-pixel panel"
    );
    assert!(
        !readout
            .tints()
            .iter()
            .any(|entry| entry.ends_with("#9a927e")),
        "exactly twelve rows have no scrollbar"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.contains("ui/font/tiny/") && entry.ends_with(" 16"))
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.contains("ui/font/small/") && entry.ends_with(" 18"))
    );
}

#[test]
fn timer_lock_progress_and_leaving_before_lock_are_exact_and_locked_spans_have_hits() {
    let mut readout = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("timer_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("timer_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("timer_target", function() return {{ {text = "link", tooltip = function() return {"child"} end} }} end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
        TooltipsConfig {
            delay: 0.3,
            lock: "timer",
            lock_timer: 1.0,
        },
    );
    for _ in 0..19 {
        readout.update(
            800,
            600,
            Some(pointer_packet("timer_target", 32.0, Vec::new())),
        );
    }
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry.contains("ui/theme/cut3_tl_fill.png"))
    );
    assert!(
        !readout
            .tints()
            .iter()
            .any(|entry| entry.ends_with("#f0a830")),
        "the bar is empty when the panel opens"
    );
    for _ in 0..31 {
        readout.update(
            800,
            600,
            Some(pointer_packet("timer_target", 32.0, Vec::new())),
        );
    }
    let tints = readout.tints();
    let half_bar = tints
        .iter()
        .rev()
        .find(|entry| entry.ends_with("#f0a830"))
        .expect("the half-filled bar");
    let bar_id = half_bar.split_whitespace().nth(1).unwrap();
    let entries = readout.entries();
    let geometry = entries
        .iter()
        .rev()
        .find(|entry| {
            entry.starts_with(&format!("create {bar_id} "))
                || entry.starts_with(&format!("set_rect {bar_id} "))
        })
        .expect("bar geometry");
    let half_width = geometry
        .split_whitespace()
        .nth(if geometry.starts_with("create ") {
            5
        } else {
            4
        })
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let panel = tooltip_panel_rect(&readout).expect("the unlocked tooltip panel");
    let panel_width = panel.2 / 2;
    assert_eq!(
        half_width,
        ((panel_width - 1) / 2) * 2,
        "at 0.5 s the fill is exactly half the inner width: {half_bar}, {geometry}, panel={panel:?}"
    );
    for _ in 0..25 {
        readout.update(
            800,
            600,
            Some(pointer_packet("timer_target", 32.0, Vec::new())),
        );
    }
    let before_leave = readout.entries().len();
    readout.update(800, 600, Some(pointer_packet("outside", 32.0, Vec::new())));
    assert!(
        readout.entries()[before_leave..]
            .iter()
            .any(|entry| entry.starts_with("destroy q")),
        "leaving at 0.9 s closes"
    );
}

#[test]
fn a_timer_locked_tooltip_keeps_panel_and_span_hits() {
    let mut locked = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("timer_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("timer_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("timer_target", function() return {{{text = "link", tooltip = function() return {"child"} end}}} end)
frame.on_update(function(update) tooltip.update(update) end)
"#,
        TooltipsConfig {
            delay: 0.3,
            lock: "timer",
            lock_timer: 1.0,
        },
    );
    for _ in 0..82 {
        locked.update(
            800,
            600,
            Some(pointer_packet("timer_target", 32.0, Vec::new())),
        );
    }
    assert_eq!(current_hits(&locked.entries(), "tooltip_panel_").len(), 1);
    assert_eq!(
        current_hits(&locked.entries(), "tooltip_span_").len(),
        1,
        "locked link publishes its own key"
    );
}

#[test]
fn middle_mouse_locks_with_a_false_key_but_disabled_and_timer_ignore_middle() {
    let script = r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
pointer.register("middle_target", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("middle_target", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.attach("middle_target", function() return {"middle"} end)
frame.on_update(function(update) tooltip.update(update) end)
"#;
    let middle = ConfigValue::Table(BTreeMap::from([
        (
            "kind".to_owned(),
            ConfigValue::String("button_down".to_owned()),
        ),
        (
            "button".to_owned(),
            ConfigValue::String("middle".to_owned()),
        ),
        ("key".to_owned(), ConfigValue::Bool(false)),
        ("x".to_owned(), ConfigValue::Float(32.0)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("inside".to_owned(), ConfigValue::Bool(false)),
    ]));
    let mut manual = Readout::script_with_tooltips(
        script,
        TooltipsConfig {
            delay: 0.3,
            lock: "middle_mouse",
            lock_timer: 1.0,
        },
    );
    for _ in 0..19 {
        manual.update(
            800,
            600,
            Some(pointer_packet("middle_target", 32.0, Vec::new())),
        );
    }
    assert!(
        !manual
            .tints()
            .iter()
            .any(|entry| entry.ends_with("#f0a830")),
        "middle_mouse has no unlocked bar"
    );
    manual.update(
        800,
        600,
        Some(pointer_packet("middle_target", 32.0, vec![middle.clone()])),
    );
    assert!(current_hits(&manual.entries(), "tooltip_panel_").len() == 1);
    assert!(
        manual
            .tints()
            .iter()
            .any(|entry| entry.ends_with("#f0a830")),
        "false-key middle press locks immediately"
    );

    for lock in ["timer", "disabled"] {
        let mut readout = Readout::script_with_tooltips(
            script,
            TooltipsConfig {
                delay: 0.3,
                lock,
                lock_timer: 1.0,
            },
        );
        for _ in 0..19 {
            readout.update(
                800,
                600,
                Some(pointer_packet("middle_target", 32.0, Vec::new())),
            );
        }
        readout.update(
            800,
            600,
            Some(pointer_packet("middle_target", 32.0, vec![middle.clone()])),
        );
        assert!(
            current_hits(&readout.entries(), "tooltip_panel_").is_empty(),
            "{lock} does not middle-lock"
        );
        if lock == "disabled" {
            for _ in 0..70 {
                readout.update(
                    800,
                    600,
                    Some(pointer_packet("middle_target", 32.0, Vec::new())),
                );
            }
            assert!(
                !readout
                    .tints()
                    .iter()
                    .any(|entry| entry.ends_with("#f0a830")),
                "disabled draws no bar"
            );
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one nested scenario opens A/B/C then checks functional, descendant, map and Escape closing against that stack"
)]
fn nested_tooltips_reach_three_levels_and_click_rules_close_the_requested_depth() {
    let mut readout = Readout::script_with_tooltips(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local windows = require("lib.window")
local key_window = windows.new{title = "Keep open", content_width = 100, content_height = 60}
window.record("frame_key " .. key_window.frame_key)
local button = require("lib.button")
local action_button = button.new{x = 200, y = 20, text = "OK", on_click = function() end}
window.record("action_button " .. action_button.key)
pointer.register("root", {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit("root", {{x = 20, y = 20, width = 80, height = 24}})
tooltip.define("A", function() return {{{text = "to B", class = "B", args = {}}}} end)
tooltip.define("B", function() return {{{text = "to C", class = "C", args = {}}}} end)
tooltip.define("C", function() return {"C body"} end)
tooltip.attach("root", {class = "A", args = {}})
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
  if update.pointer.over == "close_key" then
    window.record("key_close " .. tostring(windows.close_request("key")) .. " " .. tostring(key_window.closed))
  elseif update.pointer.over == "escape_top" then
    window.record("top " .. tostring(tooltip.close_top()))
  end
end)
"#,
        TooltipsConfig {
            delay: 0.3,
            lock: "timer",
            lock_timer: 1.0,
        },
    );
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("root", 32.0, Vec::new())));
    }
    let a_span = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .next()
        .unwrap()
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&a_span, 52.0, Vec::new())));
    }
    let b_span = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_2"))
        .unwrap()
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&b_span, 72.0, Vec::new())));
    }
    assert_eq!(
        current_hits(&readout.entries(), "tooltip_panel_").len(),
        3,
        "A, B, C are all locked"
    );

    let functional_key = records(&readout)
        .iter()
        .find_map(|record| record.strip_prefix("action_button "))
        .expect("the functional button key")
        .to_owned();
    let functional_down = pointer_event("down", Some(&functional_key), None);
    readout.update(
        800,
        600,
        Some(pointer_packet(
            &functional_key,
            202.0,
            vec![functional_down],
        )),
    );
    assert_eq!(
        current_hits(&readout.entries(), "tooltip_panel_").len(),
        3,
        "a functional press closes nothing"
    );

    let panel_down = pointer_event("down", Some("tooltip_panel_2"), None);
    readout.update(
        800,
        600,
        Some(pointer_packet("tooltip_panel_2", 45.0, vec![panel_down])),
    );
    let retained = current_hits(&readout.entries(), "tooltip_panel_");
    assert!(retained.contains_key("tooltip_panel_1") && retained.contains_key("tooltip_panel_2"));
    assert!(
        !retained.contains_key("tooltip_panel_3"),
        "a press on B closes only its child C"
    );

    let map_down = pointer_event("map_down", None, None);
    readout.update(
        800,
        600,
        Some(pointer_packet("outside", 400.0, vec![map_down])),
    );
    assert!(
        current_hits(&readout.entries(), "tooltip_panel_").is_empty(),
        "a map press closes all"
    );

    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("root", 32.0, Vec::new())));
    }
    readout.update(
        800,
        600,
        Some(pointer_packet("close_key", 400.0, Vec::new())),
    );
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry == "record key_close false false")
    );

    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("root", 32.0, Vec::new())));
    }
    let a_span = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_5"))
        .unwrap()
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&a_span, 52.0, Vec::new())));
    }
    let b_span = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_6"))
        .unwrap()
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&b_span, 72.0, Vec::new())));
    }
    assert_eq!(current_hits(&readout.entries(), "tooltip_panel_").len(), 3);
    for _ in 0..4 {
        readout.update(
            800,
            600,
            Some(pointer_packet("escape_top", 400.0, Vec::new())),
        );
    }
    let entries = readout.entries();
    let closes: Vec<_> = entries
        .iter()
        .filter(|entry| entry.starts_with("record top "))
        .map(String::as_str)
        .collect();
    assert_eq!(
        &closes[closes.len() - 4..],
        [
            "record top true",
            "record top true",
            "record top true",
            "record top false"
        ]
    );

    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet("root", 32.0, Vec::new())));
    }
    let a_span = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_8"))
        .expect("the eighth opened A link")
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&a_span, 52.0, Vec::new())));
    }
    let b_span = current_hits(&readout.entries(), "tooltip_span_")
        .keys()
        .find(|key| key.contains("tooltip_panel_9"))
        .expect("the nested B link")
        .clone();
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer_packet(&b_span, 72.0, Vec::new())));
    }
    assert_eq!(current_hits(&readout.entries(), "tooltip_panel_").len(), 3);
    let all_records = records(&readout);
    let frame_key = all_records
        .iter()
        .find_map(|record| record.strip_prefix("frame_key "))
        .expect("the window frame key")
        .to_owned();
    readout.update(
        800,
        600,
        Some(pointer_packet(
            &frame_key,
            400.0,
            vec![pointer_event("down", Some(&frame_key), None)],
        )),
    );
    assert!(
        current_hits(&readout.entries(), "tooltip_panel_").is_empty(),
        "a keyed non-functional frame press closes every open tooltip"
    );
}

#[test]
fn pointer_functional_matches_each_actionable_widget_and_rejects_passive_regions() {
    let readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local window_api = require("lib.window")
local button = require("lib.button")
local list = require("lib.list")
local cards = require("lib.cards")
local tabs = require("lib.tabs")
local down = {kind = "down", inside = true}
local w = window_api.new{title = "Test", content_width = 160, content_height = 120}
local fixed = window_api.new{title = "Fixed", fixed = true, draggable = false, content_width = 100, content_height = 60}
local fixed_action = window_api.new{title = "Action", fixed = true, draggable = false, on_title_click = function() end, content_width = 100, content_height = 60}
local b_yes = button.new{x = 300, y = 20, text = "yes", on_click = function() end}
local b_no = button.new{x = 300, y = 50, text = "no"}
local rows = list.new{window = w, x = 0, y = 0, width = 100, rows = 1, items = {"row"}, on_select = function() end}
local passive_rows = list.new{window = w, x = 0, y = 30, width = 100, rows = 1, items = {"row"}}
local cards_widget = cards.new{window = w, x = 0, y = 60, width = 100, height = 20, gap = 1, items = {
  {height = 10, draw = function() return {} end, on_click = function() end},
  {height = 10, draw = function() return {} end},
}}
local tabs_widget = tabs.new{window = w, items = {{key = "one", label = "One"}, {key = "two", label = "Two"}}}
assert(pointer.functional(b_yes.key, down))
assert(not pointer.functional(b_no.key, down))
assert(pointer.functional(rows.row_keys[1], down))
assert(not pointer.functional(passive_rows.area_key, down))
local card_click, card_passive = false, false
for key in pairs(cards_widget.item_keys) do
  if key:match("_1_1$") then card_click = pointer.functional(key, down) end
  if key:match("_1_2$") then card_passive = pointer.functional(key, down) end
end
assert(card_click and not card_passive)
assert(pointer.functional(tabs_widget.buttons[1].key, down))
assert(pointer.functional(w.title_key, down) and not pointer.functional(w.frame_key, down))
assert(not pointer.functional(fixed.title_key, down) and pointer.functional(fixed_action.title_key, down))
assert(pointer.functional(w.close_button.key, down))
window.record("functional contract passed")
"#,
    );
    assert!(
        readout
            .entries()
            .iter()
            .any(|entry| entry == "record functional contract passed")
    );
}

#[test]
fn pointer_dispatch_classifies_a_press_before_its_widget_retires() {
    let readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local function register(key, can_act)
  local widget = {
    on_event = function(self, event)
      if event.kind == "down" then
        window.record(key .. " " .. tostring(event.functional))
        pointer.unregister(key)
      end
    end,
    on_hover = function() end,
    on_overflow = function() end,
  }
  if can_act then widget.functional = function() return true end end
  pointer.register(key, widget)
end
register("active", true)
register("passive", false)
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  window.record("missing " .. tostring(update.pointer.events[3].functional))
end)
"#,
    );
    let event = |key: &str| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String("down".to_owned())),
            ("key".to_owned(), ConfigValue::String(key.to_owned())),
            ("x".to_owned(), ConfigValue::Float(10.0)),
            ("y".to_owned(), ConfigValue::Float(10.0)),
            ("inside".to_owned(), ConfigValue::Bool(true)),
        ]))
    };
    let pointer = ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Float(10.0)),
        ("y".to_owned(), ConfigValue::Float(10.0)),
        ("held".to_owned(), ConfigValue::Bool(true)),
        ("overflow".to_owned(), ConfigValue::Bool(false)),
        ("over".to_owned(), ConfigValue::String("active".to_owned())),
        (
            "events".to_owned(),
            ConfigValue::Array(vec![event("active"), event("passive"), event("missing")]),
        ),
    ]));
    let mut readout = readout;
    readout.update(800, 600, Some(pointer));
    let entries = readout.entries();
    assert!(
        entries.iter().any(|entry| entry == "record active true"),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|entry| entry == "record passive false"),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|entry| entry == "record missing false"),
        "{entries:?}"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one long-tooltip scroll scenario checks retained span keys across overlapping viewport rows"
)]
fn a_locked_long_tooltip_keeps_visible_line_targets_when_scrolled() {
    let mut readout = Readout::script(
        r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local key = "scroll_target"
pointer.register(key, {on_event = function() end, on_hover = function() end, on_overflow = function() end})
render.set_hit(key, {{x = 20, y = 20, width = 80, height = 24}})
local lines = {}
for index = 1, 20 do
  lines[index] = {{text = "line " .. index, class = "line", args = {index = index}}}
end
tooltip.define("long", function() return lines end)
tooltip.define("line", function(args) return {tostring(args.index)} end)
tooltip.attach(key, {class = "long", args = {}})
frame.on_update(function(update)
  pointer.dispatch(update.pointer)
  tooltip.update(update)
end)
"#,
    );
    let pointer = |over: &str, events: Vec<ConfigValue>| {
        ConfigValue::Table(BTreeMap::from([
            ("x".to_owned(), ConfigValue::Float(32.0)),
            ("y".to_owned(), ConfigValue::Float(28.0)),
            ("held".to_owned(), ConfigValue::Bool(false)),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            ("over".to_owned(), ConfigValue::String(over.to_owned())),
            ("events".to_owned(), ConfigValue::Array(events)),
        ]))
    };
    for _ in 0..82 {
        readout.update(800, 600, Some(pointer("scroll_target", Vec::new())));
    }
    let first_hits = current_hits(&readout.entries(), "tooltip_span_");
    assert_eq!(
        first_hits.len(),
        12,
        "one key per visible line: {first_hits:?}"
    );
    let scroll = ConfigValue::Table(BTreeMap::from([
        ("kind".to_owned(), ConfigValue::String("scroll".to_owned())),
        (
            "key".to_owned(),
            ConfigValue::String("tooltip_panel_1".to_owned()),
        ),
        ("x".to_owned(), ConfigValue::Float(32.0)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("inside".to_owned(), ConfigValue::Bool(true)),
        ("amount".to_owned(), ConfigValue::Integer(-3)),
        ("unit".to_owned(), ConfigValue::String("lines".to_owned())),
    ]));
    readout.update(800, 600, Some(pointer("tooltip_panel_1", vec![scroll])));
    let scrolled = current_hits(&readout.entries(), "tooltip_span_");
    let overlap: Vec<_> = first_hits
        .keys()
        .filter(|key| scrolled.contains_key(*key))
        .collect();
    assert_eq!(
        overlap.len(),
        9,
        "the nine still-visible lines keep their keys: {scrolled:?}"
    );
    assert_eq!(scrolled.len(), 12);
    let line_fourteen = scrolled
        .iter()
        .find(|(key, _)| key.contains("_14:1"))
        .expect("line 14 now has a visible span key")
        .0
        .clone();
    let before_y = scrolled[&line_fourteen]
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let second_scroll = ConfigValue::Table(BTreeMap::from([
        ("kind".to_owned(), ConfigValue::String("scroll".to_owned())),
        (
            "key".to_owned(),
            ConfigValue::String("tooltip_panel_1".to_owned()),
        ),
        ("x".to_owned(), ConfigValue::Float(32.0)),
        ("y".to_owned(), ConfigValue::Float(28.0)),
        ("inside".to_owned(), ConfigValue::Bool(true)),
        ("amount".to_owned(), ConfigValue::Integer(-3)),
        ("unit".to_owned(), ConfigValue::String("lines".to_owned())),
    ]));
    readout.update(
        800,
        600,
        Some(pointer("tooltip_panel_1", vec![second_scroll])),
    );
    let scrolled_again = current_hits(&readout.entries(), "tooltip_span_");
    assert!(
        scrolled_again.contains_key(&line_fourteen),
        "line 14 retains its key: {scrolled_again:?}"
    );
    let after_y = scrolled_again[&line_fourteen]
        .split_whitespace()
        .nth(1)
        .unwrap();
    assert_ne!(before_y, after_y, "the retained key moves with its line");

    let point_scroll = |amount: f64| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String("scroll".to_owned())),
            (
                "key".to_owned(),
                ConfigValue::String("tooltip_panel_1".to_owned()),
            ),
            ("x".to_owned(), ConfigValue::Float(32.0)),
            ("y".to_owned(), ConfigValue::Float(28.0)),
            ("inside".to_owned(), ConfigValue::Bool(true)),
            ("amount".to_owned(), ConfigValue::Float(amount)),
            ("unit".to_owned(), ConfigValue::String("points".to_owned())),
        ]))
    };
    let before_fraction = current_hits(&readout.entries(), "tooltip_span_");
    for _ in 0..2 {
        readout.update(
            800,
            600,
            Some(pointer("tooltip_panel_1", vec![point_scroll(-1.5)])),
        );
    }
    assert_eq!(
        current_hits(&readout.entries(), "tooltip_span_"),
        before_fraction,
        "two -1.5 point scrolls carry a fraction without moving a row"
    );
    for _ in 0..10 {
        readout.update(
            800,
            600,
            Some(pointer("tooltip_panel_1", vec![point_scroll(-1.5)])),
        );
    }
    let one_row = current_hits(&readout.entries(), "tooltip_span_");
    assert!(one_row.keys().any(|key| key.contains("_8:1")));
    assert!(!one_row.keys().any(|key| key.contains("_7:1")));
    readout.update(
        800,
        600,
        Some(pointer(
            "tooltip_panel_1",
            vec![ConfigValue::Table(BTreeMap::from([
                ("kind".to_owned(), ConfigValue::String("scroll".to_owned())),
                (
                    "key".to_owned(),
                    ConfigValue::String("tooltip_panel_1".to_owned()),
                ),
                ("x".to_owned(), ConfigValue::Float(32.0)),
                ("y".to_owned(), ConfigValue::Float(28.0)),
                ("inside".to_owned(), ConfigValue::Bool(true)),
                ("amount".to_owned(), ConfigValue::Integer(-500)),
                ("unit".to_owned(), ConfigValue::String("lines".to_owned())),
            ]))],
        )),
    );
    assert!(
        current_hits(&readout.entries(), "tooltip_span_")
            .keys()
            .any(|key| key.contains("_9:1"))
    );
}
