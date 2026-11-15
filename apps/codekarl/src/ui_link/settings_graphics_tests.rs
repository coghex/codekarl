//! The graphics settings tab (CK-308): the shipped settings window and supporting UI scripts run through the headless UI rig.

use super::listbar_tests::{click, point};
use super::menu_tests::settings_graphics_rig;
use super::numberfield_tests::{chars, input_applied};
use super::widget_witness_tests::points;
use ck_window::{PointerEvent, TypedEntry};

fn click_at_metrics(rig: &mut super::group_tests::Rig, name: &str, width: u32, height: u32) {
    let (x, y) = point(rig, name);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(width, height, 2);
}

#[test]
fn graphics_is_the_second_settings_tab_and_has_the_three_graphics_rows() {
    let mut rig = settings_graphics_rig(&[
        r#"expect(not menu_button("settings").inactive, "settings is available")"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.tabs.graphics ~= nil, "graphics is a settings tab")
eq(#s.tab_order, 4)
eq(s.tab_order[1], "general") eq(s.tab_order[2], "graphics")
eq(s.tab_order[3], "map") eq(s.tab_order[4], "input")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics", "the graphics tab opens")
local labels = {}
for _, line in ipairs(lines_in(s.page.group)) do labels[line_text(line)] = line end
expect(labels.resolution ~= nil and labels["UI scale up to"] ~= nil and labels["list look"] ~= nil,
  "graphics has resolution, UI scale up to, and list look")
expect(labels["window mode"] == nil and labels["frame rate"] == nil,
  "window mode and frame rate belong to later slices")
"#,
    ]);
    rig.opened();
    click(&mut rig, "settings");
    rig.opened();
    click(&mut rig, "settings_tab_graphics");
    rig.opened();
}

#[test]
fn resolution_fields_hold_five_digits_clamp_through_resize_and_ignore_old_metrics() {
    let mut rig = settings_graphics_rig(&[
        r#"
fixture.graphics.width, fixture.graphics.height, fixture.graphics.scale = 12000, 1200, 2
require("lib.settings_window").update({pixel_width=12000, pixel_height=1200})
"#,
        r#"
expect(require("lib.settings_window").current() ~= nil)
require("lib.settings_window").update({pixel_width=12000, pixel_height=1200})
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics")
eq(s.controls.resolution_width:get(), 12000, "the first drawable width is shown")
eq(s.controls.resolution_height:get(), 1200, "the first drawable height is shown")
eq(select(1, s.controls.resolution_width:size()), (font.layout("00000").width + 8) * P,
  "the width field is sized for five digits")
expect(s.controls.resolution_height.x + select(1, s.controls.resolution_height:size()) <= s.page.room,
  "the five-digit fields fit within the row")
expect(s.controls.scale[3].inactive and s.controls.scale[2].selected,
  "the current resolution selects 1.5 and greys 2")
mark_settings()
"#,
        r#"expect(require("lib.settings_window").current().controls.resolution_width:editing())"#,
        r#"
local s = require("lib.settings_window").current()
local call
for _, candidate in ipairs(fixture.calls) do if candidate[1] == "resize" then call = candidate end end
expect(call ~= nil, "applying either dimension requests resize")
eq(call[2], 99999) eq(call[3], 1200)
eq(s.controls.resolution_width:get(), 16384, "the engine's returned clamp is shown immediately")
eq(s.controls.resolution_height:get(), 1200, "the other dimension is sent with the field")
eq(fixture.graphics.scale, 1.5, "an applied size drops a limit the drawable cannot hold")
eq(s.controls.scale[2].selected, true) eq(s.controls.scale[3].inactive, true)
"#,
        r#"
local s = require("lib.settings_window").current()
require("lib.settings_window").update({pixel_width=12000, pixel_height=1200})
eq(s.controls.resolution_width:get(), 16384, "an unchanged old-size update cannot undo the returned pair")
require("lib.settings_window").update({pixel_width=16384, pixel_height=1200})
eq(s.controls.resolution_width:get(), 16384, "the later drawable metrics are followed")
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 6000, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 6000, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_resolution_width", 6000, 600);
    rig.opened();
    input_applied(&mut rig);
    rig.deliver_fixture_frame_at(6000, 600, 2);
    let mut typed = vec![TypedEntry::Backspace; 5];
    typed.extend(chars("99999"));
    typed.push(TypedEntry::Enter);
    for entry in typed {
        rig.mapper.typed(entry);
        rig.feed(&[]);
        rig.deliver_fixture_frame_at(6000, 600, 2);
    }
    rig.opened();
    rig.opened();
}

#[test]
fn applying_the_height_field_resizes_with_the_shown_width() {
    let mut rig = settings_graphics_rig(&[
        r#"
fixture.graphics.width, fixture.graphics.height = 1920, 1200
require("lib.settings_window").update({pixel_width=1920, pixel_height=1200})
"#,
        r#"
expect(require("lib.settings_window").current() ~= nil)
require("lib.settings_window").update({pixel_width=1920, pixel_height=1200})
mark_settings()
"#,
        r"mark_settings()",
        r#"expect(require("lib.settings_window").current().controls.resolution_height:editing())"#,
        r#"
local s = require("lib.settings_window").current()
local call
for _, candidate in ipairs(fixture.calls) do if candidate[1] == "resize" then call = candidate end end
expect(call ~= nil, "applying the height requests one resize")
eq(call[2], 1920) eq(call[3], 99999)
eq(s.controls.resolution_width:get(), 1920)
eq(s.controls.resolution_height:get(), 16384, "the resize service's clamped height shows immediately")
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_resolution_height", 960, 600);
    rig.opened();
    input_applied(&mut rig);
    rig.deliver_fixture_frame_at(960, 600, 2);
    let mut typed = vec![TypedEntry::Backspace; 4];
    typed.extend(chars("99999"));
    typed.push(TypedEntry::Enter);
    for entry in typed {
        rig.mapper.typed(entry);
    }
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(960, 600, 2);
    rig.opened();
}

#[test]
fn graphics_fields_show_the_used_size_when_the_screen_is_below_the_kept_minimum() {
    let mut rig = settings_graphics_rig(&[
        r#"require("lib.settings_window").update({pixel_width=800, pixel_height=600})"#,
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics")
eq(settings.graphics().width, 1280, "the settings API reports the kept minimum")
eq(s.controls.resolution_width:get(), 800, "the field shows the size actually in use")
eq(s.controls.resolution_height:get(), 600, "the field shows the size actually in use")
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 400, 300);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 400, 300);
    rig.opened();
}

#[test]
fn a_scale_choice_uses_the_returned_resolution_before_new_pixels_arrive() {
    let mut rig = settings_graphics_rig(&[
        r#"
fixture.graphics.scale = 1
require("lib.settings_window").update({pixel_width=1280, pixel_height=800})
"#,
        r#"
expect(require("lib.settings_window").current() ~= nil)
require("lib.settings_window").update({pixel_width=1280, pixel_height=800})
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
s.controls.resolution_width.buffer = "1920"
s.controls.resolution_width:commit()()
s.controls.resolution_height.buffer = "1200"
s.controls.resolution_height:commit()()
expect(not s.controls.scale[2].inactive, "the returned size makes 1.5 live immediately")
s.controls.scale[2].on_click()
eq(fixture.graphics.scale, 1.5, "the returned size permits the scale choice before metrics arrive")
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.resolution_width:get(), 1920, "refreshing the tab keeps the pending width")
eq(s.controls.resolution_height:get(), 1200, "refreshing the tab keeps the pending height")
eq(s.controls.scale[2].selected, true)
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 640, 400);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 640, 400);
    rig.opened();
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
}

#[test]
fn drawable_pixel_updates_refresh_scale_choices_without_writing_a_limit() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.scale = nil",
        r#"
expect(require("lib.settings_window").current() ~= nil)
require("lib.settings_window").update({pixel_width=1280, pixel_height=800})
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics")
eq(s.controls.scale[1].selected, true)
expect(s.controls.scale[2].inactive and s.controls.scale[3].inactive)
"#,
        r#"
local s = require("lib.settings_window").current()
require("lib.settings_window").update({width=640, height=400, pixel_width=1920, pixel_height=1200})
eq(s.controls.resolution_width:get(), 1920)
eq(s.controls.scale[2].selected, true)
expect(s.controls.scale[3].inactive)
eq(fixture.graphics.scale, nil, "manual size changes do not write the implicit limit")
require("lib.settings_window").update({width=640, height=400, pixel_width=2560, pixel_height=1600})
eq(s.controls.resolution_width:get(), 2560)
eq(s.controls.scale[3].selected, true)
expect(not s.controls.scale[3].inactive)
eq(#fixture.calls, 0, "reading drawable pixels writes no setting")
mark_settings()
"#,
        r#"
eq(fixture.graphics.scale, 2, "choosing a live scale stores the setting")
eq(fixture.calls[#fixture.calls][1], "set")
eq(fixture.calls[#fixture.calls][2], "ui.scale")
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 640, 400);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 640, 400);
    rig.opened();
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_scale_3", 1280, 800);
    rig.opened();
}

#[test]
fn list_look_is_kept_and_graphics_defaults_reset_the_tab_with_one_resize() {
    let mut rig = settings_graphics_rig(&[
        r#"
fixture.graphics = {width=1920, height=1200, list_look="cards", scale=1.5}
unsaved_marks = 0
local mark_unsaved = game.mark_unsaved
game.mark_unsaved = function()
  unsaved_marks = unsaved_marks + 1
  mark_unsaved()
end
require("lib.settings_window").update({pixel_width=1920, pixel_height=1200})
"#,
        r#"
expect(require("lib.settings_window").current() ~= nil)
require("lib.settings_window").update({pixel_width=1920, pixel_height=1200})
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics")
eq(s.controls.resolution_width:get(), 1920)
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(fixture.graphics.list_look, "rows", "the button writes the shared setting")
eq(require("lib.list_look").get(), "rows")
eq(s.controls.list_look[2].selected, true)
eq(unsaved_marks, 1, "an effective graphics-tab look change marks the game unsaved once")
mark_settings()
"#,
        r#"eq(unsaved_marks, 1, "reselecting the current graphics look does not mark unsaved again")"#,
        r#"
local s = require("lib.settings_window").current()
eq(fixture.calls[#fixture.calls][1], "resize", "defaults asks the resize service")
eq(fixture.graphics.width, 1280) eq(fixture.graphics.height, 800)
eq(fixture.graphics.list_look, "cards") eq(fixture.graphics.scale, nil)
eq(fixture.calls[#fixture.calls][1], "resize")
eq(fixture.calls[#fixture.calls][2], 1280) eq(fixture.calls[#fixture.calls][3], 800)
local resize_count = 0
for _, call in ipairs(fixture.calls) do if call[1] == "resize" then resize_count = resize_count + 1 end end
eq(resize_count, 1, "defaults request exactly one resize")
eq(s.controls.resolution_width:get(), 1280)
eq(s.controls.resolution_height:get(), 800)
eq(s.controls.list_look[1].selected, true)
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_list_look_2", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_list_look_2", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_defaults", 960, 600);
    rig.opened();
}

#[test]
fn list_look_choice_without_a_game_is_saved_and_keeps_the_ui_run_alive() {
    let mut rig = settings_graphics_rig(&[
        r#"
game = nil
expect(not menu_button("settings").inactive, "settings remains available without a game")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s ~= nil, "opening settings without a game succeeds")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics", "the graphics tab remains reachable without a game")
mark_settings()
"#,
        r#"
eq(fixture.graphics.list_look, "rows", "the click writes the shared setting without a game")
eq(game, nil)
"#,
    ]);
    rig.opened();
    click_at_metrics(&mut rig, "settings", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_tab_graphics", 960, 600);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_list_look_2", 960, 600);
    rig.opened();
}
