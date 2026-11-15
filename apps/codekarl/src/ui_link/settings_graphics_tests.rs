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
fn graphics_is_the_second_settings_tab_and_has_window_mode_and_frame_rate_rows() {
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
expect(labels.resolution ~= nil and labels["window mode"] ~= nil and labels["UI scale up to"] ~= nil
  and labels["frame rate"] ~= nil and labels["list look"] ~= nil,
  "graphics has resolution, window mode, UI scale up to, frame rate, and list look")
expect(labels["frame rate"] ~= nil, "graphics has the frame rate row (CK-313)")
eq(#s.controls.window_mode, 3, "the mode row has three choices")
eq(s.controls.window_mode[1].y, 20 * P, "the mode row is 20 below the resolution row")
eq(s.controls.scale[1].y, s.controls.window_mode[#s.controls.window_mode].y + 20 * P,
  "the scale row begins five pixels after the mode buttons")
for _, control in ipairs(s.controls.window_mode) do
  expect(control.x + control.width <= s.page.room, "each window mode button stays inside the content edge")
end
"#,
    ]);
    rig.opened();
    click(&mut rig, "settings");
    rig.opened();
    click(&mut rig, "settings_tab_graphics");
    rig.opened();
}

#[test]
fn window_mode_choices_update_the_mode_and_deactivate_resolution_fields_only_in_borderless_mode() {
    let mut rig = settings_graphics_rig(&[
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
s.controls.window_mode[2].on_click()
eq(fixture.calls[#fixture.calls][1], "set")
eq(fixture.calls[#fixture.calls][2], "window.mode")
eq(fixture.calls[#fixture.calls][3], "windowed_fullscreen")
eq(s.controls.window_mode[2].selected, true)
fixture.graphics.width, fixture.graphics.height = 1280, 800
require("lib.settings_window").update({pixel_width=1920, pixel_height=1200})
eq(s.controls.resolution_width:get(), 1920, "windowed fullscreen shows the screen drawable width")
eq(s.controls.resolution_height:get(), 1200, "windowed fullscreen shows the screen drawable height")
expect(s.controls.resolution_width.inactive and s.controls.resolution_height.inactive,
  "windowed fullscreen greys both resolution fields")
expect(hit_rect(s.controls.resolution_width.key) == nil, "a greyed field publishes no hit region")
eq(s.controls.scale[2].selected, true, "screen drawable selects its attainable scale")
expect(s.controls.scale[3].inactive, "screen drawable greys a scale it cannot hold")
s.controls.window_mode[3].on_click()
eq(fixture.graphics.mode, "fullscreen")
eq(s.controls.window_mode[3].selected, true)
expect(not s.controls.resolution_width.inactive and not s.controls.resolution_height.inactive,
  "fullscreen keeps resolution fields live")
s.controls.window_mode[1].on_click()
eq(fixture.graphics.mode, "windowed")
eq(s.controls.window_mode[1].selected, true)
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
expect(s.controls.resolution_height:editing(), "Tab starts height after the resize callback")
expect(s.controls.resolution_height.caret ~= nil
  and s.controls.resolution_height.frame.line == theme_lib.kind("window").hot,
  "the destination keeps its caret and outline after the resolution layout update")
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
    typed.push(TypedEntry::Tab);
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
fixture.graphics = {width=1920, height=1200, list_look="cards", scale=1.5, mode="fullscreen"}
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
eq(fixture.graphics.mode, "windowed", "defaults restores the saved window mode")
eq(fixture.calls[#fixture.calls][1], "resize")
eq(fixture.calls[#fixture.calls][2], 1280) eq(fixture.calls[#fixture.calls][3], 800)
local resize_count = 0
for _, call in ipairs(fixture.calls) do if call[1] == "resize" then resize_count = resize_count + 1 end end
eq(resize_count, 1, "defaults request exactly one resize")
eq(s.controls.resolution_width:get(), 1280)
eq(s.controls.resolution_height:get(), 800)
eq(s.controls.list_look[1].selected, true)
eq(s.controls.window_mode[1].selected, true, "defaults lights the windowed choice")
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

/// Opens the settings window on its graphics tab: three steps (the first two before the tab's own).
fn open_graphics(rig: &mut super::group_tests::Rig) {
    rig.opened();
    click_at_metrics(rig, "settings", 640, 400);
    rig.opened();
    click_at_metrics(rig, "settings_tab_graphics", 640, 400);
    rig.opened();
}

fn replace_graphics_field_and_navigate(
    rig: &mut super::group_tests::Rig,
    control: Option<&str>,
    backspaces: usize,
    digits: &str,
    navigation: TypedEntry,
) {
    if let Some(control) = control {
        click_at_metrics(rig, control, 640, 400);
        rig.opened();
        input_applied(rig);
        rig.deliver_fixture_frame_at(640, 400, 2);
    }
    for _ in 0..backspaces {
        rig.mapper.typed(TypedEntry::Backspace);
    }
    for entry in chars(digits) {
        rig.mapper.typed(entry);
    }
    rig.mapper.typed(navigation);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
}

/// A press on the frame-rate field, then the entries typed into it (the field's editing is ready after one frame).
fn type_into_the_frame_rate_field(
    rig: &mut super::group_tests::Rig,
    backspaces: usize,
    digits: &str,
) {
    click_at_metrics(rig, "settings_control_fps", 640, 400);
    rig.opened();
    input_applied(rig);
    rig.deliver_fixture_frame_at(640, 400, 2);
    let mut typed = vec![TypedEntry::Backspace; backspaces];
    typed.extend(chars(digits));
    typed.push(TypedEntry::Enter);
    for entry in typed {
        rig.mapper.typed(entry);
    }
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
}

#[test]
fn the_frame_rate_row_sits_between_scale_and_list_look_with_none_lit_and_the_field_greyed() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = nil",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
expect(s.selected == "graphics")
local labels = {}
for _, line in ipairs(lines_in(s.page.group)) do labels[line_text(line)] = line end
local _, scale_y = line_at(labels["UI scale up to"])
local rate_x, rate_y = line_at(labels["frame rate"])
local _, look_y = line_at(labels["list look"])
expect(rate_y > scale_y, "the frame rate row follows the scale row")
expect(look_y > rate_y, "list look follows the frame rate row")
eq(s.controls.list_look[1].y, s.controls.fps.y + 20 * P)
local fps, none = s.controls.fps, s.controls.fps_none
eq(fps.x, 84 * P, "the field is 84 right of the label")
eq(fps.y, s.controls.scale[1].y + 20 * P)
local fps_width = select(1, fps:size())
eq(select(1, fps:size()), (font.layout("000").width + 8) * P, "the field holds three digits")
expect(labels.fps ~= nil, "fps is shown beside the field")
local fps_x = line_at(labels.fps) - rate_x -- the label's own x is the group's origin
eq(fps_x, fps.x + fps_width + 4 * P, "fps is 4 right of the field")
eq(none.x, fps_x + font.layout("fps").width * P + 6 * P, "the none button is 6 right of fps")
eq(none.y, fps.y)
expect(none.selected, "none is lit when no limit is set")
expect(fps:is_greyed(), "the field is greyed")
eq(fps:get(), 60, "a run that starts with none shows 60")
for _, control in ipairs({fps, none}) do
  local width = select(1, control:size())
  expect(control.x + width <= s.page.room, "no control of the row passes the content's right edge")
end
eq(#fixture.calls, 0, "showing the row sets nothing")
"#,
    ]);
    open_graphics(&mut rig);
}

#[test]
fn a_press_on_the_greyed_field_turns_the_limit_on_with_its_number_and_starts_editing() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = nil",
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
local fps = s.controls.fps
expect(fps:editing(), "the press starts the editing")
expect(not fps:is_greyed(), "the field is lit")
expect(not s.controls.fps_none.selected, "none is no longer lit")
eq(fixture.graphics.fps_limit, 60, "the limit is the number the field showed")
eq(fixture.calls[#fixture.calls][2], "frame.fps_limit")
eq(fixture.calls[#fixture.calls][3], 60)
"#,
    ]);
    open_graphics(&mut rig);
    click_at_metrics(&mut rig, "settings_control_fps", 640, 400);
    rig.opened();
}

#[test]
fn graphics_tab_with_a_limit_visits_width_height_fps_and_wraps_in_both_directions() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = 60",
        r"mark_settings()",
        r#"
mark_settings()
local s = require("lib.settings_window").current()
eq(s.controls.resolution_width:get(), 1280)
eq(s.controls.resolution_height:get(), 800)
eq(s.controls.fps:get(), 60)
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing()
  and not s.controls.fps:editing(), "the graphics fields begin closed")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.resolution_width:editing(), "the real graphics width field begins editing")
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.resolution_width:get(), 1400, "Tab applies width")
expect(s.controls.resolution_height:editing(), "width advances to height")
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.resolution_height:get(), 900, "Tab applies height")
expect(s.controls.fps:editing(), "height advances to the live frame-rate field")
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.fps:get(), 75, "Tab applies the frame-rate field")
eq(fixture.graphics.fps_limit, 75)
expect(s.controls.resolution_width:editing(), "the order wraps from frame rate to width")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.resolution_height:editing(), "Tab advances from width to height")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.resolution_width:editing(), "Shift+Tab from height returns to width")
expect(not s.controls.resolution_height:editing() and not s.controls.fps:editing(),
  "only the destination edits")
"#,
    ]);
    open_graphics(&mut rig);
    replace_graphics_field_and_navigate(
        &mut rig,
        Some("settings_control_resolution_width"),
        5,
        "1400",
        TypedEntry::Tab,
    );
    replace_graphics_field_and_navigate(&mut rig, None, 5, "900", TypedEntry::Tab);
    replace_graphics_field_and_navigate(&mut rig, None, 3, "75", TypedEntry::Tab);
    rig.mapper.typed(TypedEntry::Tab);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
    rig.mapper.typed(TypedEntry::Backtab);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
}

#[test]
fn graphics_tab_without_a_limit_skips_the_greyed_frame_rate_field() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = nil",
        r"mark_settings()",
        r#"
mark_settings()
local s = require("lib.settings_window").current()
expect(s.controls.fps:is_greyed(), "no limit leaves the frame-rate field greyed")
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing()
  and not s.controls.fps:editing(), "the graphics fields begin closed")
eq(fixture.graphics.fps_limit, nil)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.resolution_width:editing(), "the real graphics width field begins editing")
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.resolution_width:get(), 1400, "Tab applies width")
expect(s.controls.resolution_height:editing(), "width advances to height")
expect(s.controls.fps:is_greyed() and not s.controls.fps:editing(), "the greyed field is skipped")
eq(fixture.graphics.fps_limit, nil, "Tab does not turn on a frame-rate limit")
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.resolution_height:get(), 900, "Tab applies height")
expect(s.controls.resolution_width:editing(), "height wraps to width")
expect(s.controls.fps:is_greyed() and not s.controls.fps:editing(), "frame rate remains skipped")
eq(fixture.graphics.fps_limit, nil)
local writes = 0
for _, call in ipairs(fixture.calls) do
  if call[1] == "set" and call[2] == "frame.fps_limit" then writes = writes + 1 end
end
eq(writes, 0, "traversal never writes frame.fps_limit")
"#,
    ]);
    open_graphics(&mut rig);
    replace_graphics_field_and_navigate(
        &mut rig,
        Some("settings_control_resolution_width"),
        5,
        "1400",
        TypedEntry::Tab,
    );
    replace_graphics_field_and_navigate(&mut rig, None, 5, "900", TypedEntry::Tab);
}

#[test]
fn windowed_fullscreen_graphics_tab_keeps_its_only_live_limited_field_editing() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = 60",
        r"mark_settings()",
        r#"
mark_settings()
local s = require("lib.settings_window").current()
expect(not s.controls.resolution_width.inactive and not s.controls.resolution_height.inactive,
  "windowed mode starts with live resolution fields")
expect(not s.controls.fps:is_greyed(), "the frame-rate field is live with a limit")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.resolution_width.inactive and s.controls.resolution_height.inactive,
  "windowed fullscreen deactivates the resolution fields")
expect(not s.controls.fps:is_greyed(), "the frame-rate field remains live with a limit")
eq(fixture.graphics.fps_limit, 60)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.fps:editing(), "the live frame-rate field starts editing")
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing(),
  "inactive resolution fields are skipped")
eq(fixture.graphics.width, 1280) eq(fixture.graphics.height, 800)
eq(fixture.graphics.fps_limit, 60)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.fps:editing(), "Tab leaves the only open field editing")
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing(),
  "inactive resolution fields remain skipped")
eq(fixture.graphics.width, 1280) eq(fixture.graphics.height, 800)
eq(fixture.graphics.fps_limit, 60)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.fps:editing(), "Shift+Tab also leaves the only open field editing")
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing(),
  "inactive resolution fields remain skipped")
eq(fixture.graphics.width, 1280) eq(fixture.graphics.height, 800)
eq(fixture.graphics.fps_limit, 60)
"#,
    ]);
    open_graphics(&mut rig);
    click_at_metrics(&mut rig, "settings_control_window_mode_2", 640, 400);
    rig.opened();
    click_at_metrics(&mut rig, "settings_control_fps", 640, 400);
    rig.opened();
    input_applied(&mut rig);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.mapper.typed(TypedEntry::Tab);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
    rig.mapper.typed(TypedEntry::Backtab);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
}

#[test]
fn windowed_fullscreen_without_a_limit_has_no_field_for_tab_to_edit() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = nil",
        r"mark_settings()",
        r#"
mark_settings()
local s = require("lib.settings_window").current()
expect(not s.controls.resolution_width.inactive and not s.controls.resolution_height.inactive,
  "windowed mode starts with live resolution fields")
expect(s.controls.fps:is_greyed(), "no limit greys the frame-rate field")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls.resolution_width.inactive and s.controls.resolution_height.inactive,
  "windowed fullscreen deactivates the resolution fields")
expect(s.controls.fps:is_greyed(), "no limit greys the frame-rate field")
eq(fixture.graphics.fps_limit, nil)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing()
  and not s.controls.fps:editing(), "no field starts editing")
eq(fixture.graphics.width, 1280) eq(fixture.graphics.height, 800)
eq(fixture.graphics.fps_limit, nil)
local writes = 0
for _, call in ipairs(fixture.calls) do
  if call[1] == "set" and call[2] == "frame.fps_limit" then writes = writes + 1 end
end
eq(writes, 0, "Tab with no open field writes no frame-rate limit")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(not s.controls.resolution_width:editing() and not s.controls.resolution_height:editing()
  and not s.controls.fps:editing(), "Shift+Tab also leaves all fields closed")
eq(fixture.graphics.fps_limit, nil)
"#,
    ]);
    open_graphics(&mut rig);
    click_at_metrics(&mut rig, "settings_control_window_mode_2", 640, 400);
    rig.opened();
    rig.mapper.typed(TypedEntry::Tab);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
    rig.mapper.typed(TypedEntry::Backtab);
    rig.feed(&[]);
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
}

#[test]
fn a_typed_limit_is_applied_and_clamped_to_30_through_240() {
    for (typed, backspaces, expected) in [("30", 2, 30), ("20", 2, 30), ("999", 2, 240)] {
        let check = format!(
            r#"
local s = require("lib.settings_window").current()
eq(fixture.graphics.fps_limit, {expected}, "typing {typed} sets {expected}")
eq(s.controls.fps:get(), {expected}, "the field shows {expected}")
expect(not s.controls.fps:editing(), "applying ends the editing")
"#
        );
        let mut rig = settings_graphics_rig(&[
            r"fixture.graphics.fps_limit = 60",
            r"mark_settings()",
            r"mark_settings()",
            r#"expect(require("lib.settings_window").current().controls.fps:editing())"#,
            &check,
        ]);
        open_graphics(&mut rig);
        type_into_the_frame_rate_field(&mut rig, backspaces, typed);
        rig.opened();
    }
}

#[test]
fn a_press_on_none_sets_none_greys_the_field_and_keeps_its_number() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = 90",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(s.controls.fps:get(), 90)
expect(not s.controls.fps:is_greyed() and not s.controls.fps_none.selected, "a limit in force: lit field, none not lit")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(fixture.graphics.fps_limit, nil, "none is set")
eq(fixture.calls[#fixture.calls][3], "none")
expect(s.controls.fps:is_greyed(), "the field is greyed")
expect(s.controls.fps_none.selected, "none is lit")
eq(s.controls.fps:get(), 90, "the field keeps the last limit")
"#,
    ]);
    open_graphics(&mut rig);
    click_at_metrics(&mut rig, "settings_control_fps_none", 640, 400);
    rig.opened();
}

#[test]
fn graphics_defaults_put_the_frame_rate_limit_back_to_none() {
    let mut rig = settings_graphics_rig(&[
        r"fixture.graphics.fps_limit = 45",
        r"mark_settings()",
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(fixture.graphics.fps_limit, nil, "defaults put the limit back")
expect(s.controls.fps_none.selected and s.controls.fps:is_greyed(), "none is lit and the field greyed again")
eq(s.controls.fps:get(), 45, "the field keeps the last number for the run")
"#,
    ]);
    open_graphics(&mut rig);
    click_at_metrics(&mut rig, "settings_control_defaults", 640, 400);
    rig.opened();
}
