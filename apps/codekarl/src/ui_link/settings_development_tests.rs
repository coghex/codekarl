//! The development settings page and the debug layer's row on the input tab (CK-336): the shipped settings window and
//! supporting UI scripts run through the headless UI rig, against the rig's settings double (which takes and refuses what
//! the host's `debug.level`, `debug.rate` and `reset("debug")` take and refuse).

use super::listbar_tests::{click, frame};
use super::menu_tests::{click_marked, settings_graphics_rig};

/// Lua shared by the steps: the lit choice of a row, and a row's button texts.
const HELPERS: &str = r#"
function lit(row)
  local shown = {}
  for index, control in ipairs(row) do if control.selected then shown[#shown + 1] = line_text(control.line) end end
  return table.concat(shown, ",")
end
function texts(row)
  local shown = {}
  for index, control in ipairs(row) do shown[index] = line_text(control.line) end
  return table.concat(shown, ",")
end
"#;

fn development_rig(steps: &[&str]) -> super::group_tests::Rig {
    // The helpers are defined in the first step so every later step can use them.
    let mut all = vec![];
    let first = format!("{HELPERS}\n{}", steps[0]);
    all.push(first.as_str());
    all.extend(steps[1..].iter().copied());
    settings_graphics_rig(&all)
}

fn open_development(rig: &mut super::group_tests::Rig) {
    rig.opened();
    click(rig, "settings");
    rig.opened();
    click(rig, "settings_tab_development");
    rig.opened();
}

#[test]
fn development_is_the_fifth_tab_and_the_window_still_fits_the_smallest_view() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(#s.tab_order, 5)
eq(s.tab_order[1], "general") eq(s.tab_order[2], "graphics") eq(s.tab_order[3], "map")
eq(s.tab_order[4], "input") eq(s.tab_order[5], "development")
expect(s.tabs.development ~= nil, "development is a tab")
local widest = 0
for _, name in ipairs(s.tab_order) do widest = math.max(widest, font.layout(name).width) end
eq(widest, font.layout("development").width, "development is the longest tab name")
eq(s.tab_width, (widest + 12) * P, "the column takes the new name's width")
eq(s.tabs.development.y - s.tabs.input.y, 13 * P, "the new tab is 13 below input")
eq(s.tabs.development.width, s.tab_width - 3 * P)
s:relayout({width = 640, height = 400})
local x, y = s.window:position()
local w, h = s.window:size()
expect(x >= 0 and y >= 0 and x + w <= 640 and y + h <= 400, "the window lies inside a 640 by 400 view")
expect(math.abs(x + w / 2 - 320) <= P and math.abs(y + h / 2 - 200) <= P, "and is centred in it")
s:relayout({width = 800, height = 600})
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "development", "a click on the tab shows the page")
expect(s.tabs.development.selected and not s.tabs.input.selected, "the tab is the lit one")
"#,
    ]);
    rig.opened();
    click(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_development");
    rig.opened();
}

#[test]
fn the_page_shows_off_and_frames_with_the_level_label_whole_and_every_row_inside_the_content() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "development")
eq(texts(s.controls.debug_level), "off,1")
eq(lit(s.controls.debug_level), "off", "the level in force is lit: 0 is off")
eq(texts(s.controls.debug_rate), "frames,updates")
eq(lit(s.controls.debug_rate), "frames")
local labels = {}
for _, line in ipairs(lines_in(s.page.group)) do labels[line_text(line)] = line end
expect(labels["debug layer level"] ~= nil, "the label is shown whole, with no two dots")
expect(labels["rate shown"] ~= nil, "the rate row is labelled")
local column = (math.max(font.layout("debug layer level").width, font.layout("rate shown").width) + 6) * P
eq(s.controls.debug_level[1].x, column, "the label column is the widest label plus 6")
eq(s.controls.debug_rate[1].x, column, "both rows share it")
eq(s.controls.debug_level[1].y, 0, "the level row is at the top")
eq(s.controls.debug_rate[1].y, 20 * P, "the rate row is 20 below it")
for _, row in ipairs({s.controls.debug_level, s.controls.debug_rate}) do
  for _, control in ipairs(row) do
    expect(control.x + control.width <= s.page.room, "no choice passes the content's right edge")
  end
end
for _, line in ipairs(lines_in(s.page.group)) do
  expect(line.x + line.width * P <= s.page.room + P, "no label passes the content's right edge")
end
mark_settings()
"#,
    ]);
    open_development(&mut rig);
}

#[test]
fn clicks_set_the_level_and_the_rate_through_the_settings_calls_and_light_the_choice() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
local call = fixture.calls[#fixture.calls]
eq(call[1], "set") eq(call[2], "debug.level") eq(call[3], 1)
eq(math.type(call[3]), "integer", "the level is the integer the host takes")
eq(fixture.debug.level, 1)
eq(lit(s.controls.debug_level), "1", "the clicked choice is lit")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
local call = fixture.calls[#fixture.calls]
eq(call[2], "debug.level") eq(call[3], 0)
eq(fixture.debug.level, 0)
eq(lit(s.controls.debug_level), "off")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
local call = fixture.calls[#fixture.calls]
eq(call[1], "set") eq(call[2], "debug.rate") eq(call[3], "updates")
eq(fixture.debug.rate, "updates")
eq(lit(s.controls.debug_rate), "updates")
eq(lit(s.controls.debug_level), "off", "the rate row is the same whatever the level is")
for _, control in ipairs(s.controls.debug_rate) do expect(not control.inactive, "the rate row is never greyed") end
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
local call = fixture.calls[#fixture.calls]
eq(call[2], "debug.rate") eq(call[3], "frames")
eq(fixture.debug.rate, "frames")
eq(lit(s.controls.debug_rate), "frames")
"#,
    ]);
    open_development(&mut rig);
    click_marked(&mut rig, "settings_control_debug_level_2");
    rig.opened();
    click_marked(&mut rig, "settings_control_debug_level_1");
    rig.opened();
    click_marked(&mut rig, "settings_control_debug_rate_2");
    rig.opened();
    click_marked(&mut rig, "settings_control_debug_rate_1");
    rig.opened();
}

#[test]
fn a_level_or_rate_changed_by_the_key_lights_the_other_choice_by_the_next_update() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(lit(s.controls.debug_level), "off")
-- The debug key steps the level in the host, outside the window (layer-line rule 2).
fixture.debug.level = 1
"#,
        r#"
local s = require("lib.settings_window").current()
eq(lit(s.controls.debug_level), "1", "the level choice follows the key with no click")
eq(lit(s.controls.debug_rate), "frames")
fixture.debug.rate = "updates"
fixture.debug.level = 0
"#,
        r#"
local s = require("lib.settings_window").current()
eq(lit(s.controls.debug_rate), "updates", "the rate row follows too")
eq(lit(s.controls.debug_level), "off")
for _, call in ipairs(fixture.calls) do
  expect(call[1] ~= "set", "following the key writes nothing back")
end
"#,
    ]);
    open_development(&mut rig);
    frame(&mut rig);
    rig.opened();
    frame(&mut rig);
    rig.opened();
}

#[test]
fn defaults_calls_reset_debug_and_shows_what_the_reset_restored() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(lit(s.controls.debug_level), "1")
eq(lit(s.controls.debug_rate), "updates")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
local call = fixture.calls[#fixture.calls]
eq(call[1], "reset") eq(call[2], "debug")
eq(s.selected, "development", "defaults keeps the page")
eq(lit(s.controls.debug_level), "off", "the page shows the configuration's level")
eq(lit(s.controls.debug_rate), "frames", "and its rate")
eq(fixture.debug.level, 0)
eq(fixture.debug.rate, "frames")
"#,
    ]);
    open_development(&mut rig);
    click_marked(&mut rig, "settings_control_debug_level_2");
    rig.opened();
    click_marked(&mut rig, "settings_control_debug_rate_2");
    rig.opened();
    click_marked(&mut rig, "settings_control_defaults");
    rig.opened();
}

#[test]
fn defaults_restores_the_configurations_own_debug_values() {
    let mut rig = development_rig(&[
        r#"
fixture.base_debug = {level = 1, rate = "updates"}
fixture.debug = {level = 1, rate = "updates"}
"#,
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(lit(s.controls.debug_level), "1")
eq(lit(s.controls.debug_rate), "updates")
s.controls.debug_level[1].on_click()
s.controls.debug_rate[1].on_click()
eq(fixture.debug.level, 0)
eq(fixture.debug.rate, "frames")
eq(lit(s.controls.debug_level), "off")
eq(lit(s.controls.debug_rate), "frames")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(fixture.calls[#fixture.calls][2], "debug")
eq(lit(s.controls.debug_level), "1", "the page shows the level of the configuration's [debug]")
eq(lit(s.controls.debug_rate), "updates", "and its rate")
"#,
    ]);
    open_development(&mut rig);
    click_marked(&mut rig, "settings_control_defaults");
    rig.opened();
}

#[test]
fn the_double_refuses_what_the_host_refuses() {
    let mut rig = development_rig(&[r#"
for _, pair in ipairs({{"debug.level", 2}, {"debug.level", -1}, {"debug.level", 0.5}, {"debug.level", 1.0},
    {"debug.level", "1"}, {"debug.level", "off"}, {"debug.rate", "seconds"}, {"debug.rate", 1}}) do
  local ok = pcall(settings.set, pair[1], pair[2])
  expect(not ok, "the double refuses " .. pair[1] .. " = " .. tostring(pair[2]))
end
expect(pcall(settings.set, "debug.level", 1) and settings.debug().level == 1, "and takes 1")
expect(pcall(settings.set, "debug.rate", "updates") and settings.debug().rate == "updates", "and takes updates")
settings.reset("debug")
eq(settings.debug().level, 0) eq(settings.debug().rate, "frames")
"#]);
    rig.opened();
}

#[test]
fn the_debug_layer_row_sits_after_fullscreen_and_before_menu_and_rebinds_like_fullscreen() {
    let mut rig = development_rig(&[
        r#"
for index, action in ipairs(fixture.bindings) do
  if action.name == "game_quickload" then
    table.insert(fixture.bindings, index + 1, {name = "window_fullscreen", bindings = {"F11"}})
    break
  end
end
fixture.base_bindings.window_fullscreen = {"F11"}
"#,
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
local rows = s.controls.key_slots
expect(rows.debug_layer ~= nil, "the input tab has a row for the debug layer action")
eq(#rows.debug_layer, 2, "it has the two slots every row has")
eq(line_text(rows.debug_layer[1].line), "F3")
expect(rows.window_fullscreen[1].y < rows.debug_layer[1].y and rows.debug_layer[1].y < rows.close[1].y,
  "the row is after fullscreen and before menu")
eq(rows.debug_layer[1].y - rows.window_fullscreen[1].y, 15 * P, "one row after fullscreen")
eq(rows.close[1].y - rows.debug_layer[1].y, 15 * P, "one row before menu")
local named
for _, line in ipairs(lines_in(s.page.rows_group)) do
  if line_text(line) == "debug layer" then named = line end
end
expect(named ~= nil, "the row is labelled debug layer, whole")
expect(not s.controls.rows.debug_layer.read_only, "the row is not read-only")
s.page:set_scroll(rows.debug_layer[1].y - 80 * P)
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.waiting ~= nil and s.waiting.action == "debug_layer", "a click on the slot waits for a key")
finish_capture("Control+KeyK")
eq(fixture_action("debug_layer").bindings[1], "Control+KeyK", "the capture binds the key")
local call = fixture.calls[#fixture.calls]
eq(call[1], "set_bindings")
eq(call[2].debug_layer[1], "Control+KeyK")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.waiting ~= nil and s.waiting.action == "debug_layer")
finish_capture("F5")
expect(s.prompt ~= nil, "a key another action holds opens the conflict prompt")
local lines = lines_in(s.prompt.window.content)
eq(line_text(lines[1]), "F5 is quicksave's key.")
eq(line_text(lines[2]), "Use it for debug layer instead?", "debug layer is its name in a prompt")
s.prompt.window:close()
fixture_action("debug_layer").bindings = {"F3"}
s:show("input")
s.page:set_scroll(s.controls.key_slots.game_quicksave[1].y)
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.waiting ~= nil and s.waiting.action == "game_quicksave")
finish_capture("F3")
expect(s.prompt ~= nil)
eq(line_text(lines_in(s.prompt.window.content)[1]), "F3 is debug layer's key.", "and as the owner of a key")
"#,
    ]);
    rig.opened();
    click(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_input");
    rig.opened();
    click_marked(&mut rig, "settings_slot_debug_layer_1");
    rig.opened();
    click_marked(&mut rig, "settings_slot_debug_layer_1");
    rig.opened();
    click_marked(&mut rig, "settings_slot_game_quicksave_1");
    rig.opened();
}

#[test]
fn the_input_tabs_defaults_still_resets_bindings_with_the_extra_row() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "input")
s.page:set_scroll(s.page.row_height)
expect(s.page.scroll > 0, "the rows scroll with the extra row")
fixture_action("debug_layer").bindings = {"Control+KeyK"}
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "input")
eq(s.page.scroll, 0)
eq(fixture.calls[#fixture.calls][1], "reset")
eq(fixture.calls[#fixture.calls][2], "bindings")
eq(fixture_action("debug_layer").bindings[1], "F3", "defaults restores the debug layer's key")
eq(line_text(s.controls.key_slots.debug_layer[1].line), "F3")
"#,
    ]);
    rig.opened();
    click(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_input");
    rig.opened();
    click_marked(&mut rig, "settings_control_defaults");
    rig.opened();
}

#[test]
fn every_hit_key_the_page_and_the_row_make_is_within_the_hosts_rule() {
    let mut rig = development_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r#"
local function check(where)
  local count = 0
  for key in pairs(scene.hits) do
    count = count + 1
    expect(#key >= 1 and #key <= 48 and key:match("^[a-z0-9_]+$") ~= nil,
      where .. ": hit key `" .. key .. "` is outside the host's rule (1 to 48 of a-z, 0-9, _)")
  end
  expect(count > 0, "there are hit regions to check")
end
local s = require("lib.settings_window").current()
eq(s.selected, "development")
check("development page")
s:show("input")
expect(s.controls.key_slots.debug_layer ~= nil, "the debug layer row exists")
check("input tab")
"#,
    ]);
    open_development(&mut rig);
}
