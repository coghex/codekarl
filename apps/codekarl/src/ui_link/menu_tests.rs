//! The pause menu (CK-297): `scripts/ui/lib/menu.lua` and the button library's width and inactive state, in a real UI VM over the real
//! input path, with no window and no GPU.
//!
//! The shipped library runs over the real `render` API (a Lua shadow of the scene records quads, tints, groups and hit regions), the
//! `game` extension shares one control channel and one key block with the link, and ticks come from `ck-input`'s `Mapper`. A test
//! feeds pointer events and reads what the script drew, what the simulation was sent and what the map did. Nothing sleeps: every update
//! and every posted callback is awaited.

use ck_window::{CloseSource, PointerEvent, ScrollUnit, UiEvent};

use super::group_tests::Rig;
use super::input_tick_tests::{answer, camera_of};
use super::listbar_tests::{MARKS, PROBE, click, click_at, frame, hover, point, rig};
use super::widget_witness_tests::points;
use super::*;
use crate::game::controls::Control;
use crate::game::core::Message;
use crate::game::slot::PauseReason;

/// What every test adds to the shared prelude: what `main.lua` does with a close request and with a size change, the menu's marks, and a
/// stand-in for the one host call that quits (so that a click on "quit to desktop" can be counted).
pub(super) const MENU: &str = r#"
menu = require("lib.menu")
VIEW = {width = 800, height = 600}
window.on_close_request(function(source) return menu.close_request(source, VIEW) end)
function on_frame(update)
  if update.width ~= VIEW.width or update.height ~= VIEW.height then
    VIEW.width, VIEW.height = update.width, update.height
    windows.update_viewport(VIEW.width, VIEW.height)
    menu.relayout()
  end
end
quit_calls = 0
window.close = function() quit_calls = quit_calls + 1 end
shown_paused = "unknown"
game.on_time(function(time) shown_paused = time.paused end)
function menu_button(label) return menu.current().buttons[label] end
function centre_of_button(label)
  local b = menu_button(label)
  local ox, oy = menu.current().window:content_origin()
  local w, h = b:size()
  return ox + b.x + w / 2, oy + b.y + h / 2
end
function mark_buttons()
  for _, label in ipairs(menu.LABELS) do
    local x, y = centre_of_button(label)
    mark(label, x, y)
  end
end
function dim_look(b)
  return b.frame.line == theme_lib.colour("track") and b.line.test_runs[1].colour == theme_lib.colour("dim")
    and b.frame.fill == theme_lib.colour("body")
end
"#;

pub(super) fn menu_rig(body: &str, steps: &[&str]) -> Rig {
    rig(&format!("{MENU}\n{body}"), steps)
}

/// The simulation's controls the script has sent since the last call.
pub(super) fn sent(rig: &Rig) -> Vec<Control> {
    rig.control_receiver
        .try_iter()
        .map(|message| match message {
            Message::Control(control) => control,
            other => panic!("only controls are sent: {other:?}"),
        })
        .collect()
}

pub(super) fn close_request(rig: &mut Rig, source: CloseSource) -> Answer {
    rig.link
        .post(UiEvent::CloseRequested { source })
        .expect("the close request is accepted");
    answer(&mut rig.link).answer
}

pub(super) fn click_marked(rig: &mut Rig, name: &str) {
    let at = point(rig, name);
    click_at(rig, at);
}

fn drag_marked(rig: &mut Rig, start: &str, finish: &str) {
    let from = point(rig, start);
    let to = point(rig, finish);
    rig.feed(&[PointerEvent::Down(points(from.0, from.1))]);
    rig.feed(&[PointerEvent::Move(points(to.0, to.1))]);
    rig.feed(&[PointerEvent::Up(points(to.0, to.1))]);
    frame(rig);
}

fn key_at(rig: &Rig, (x, y): (f64, f64)) -> String {
    rig.key_at(x, y)
        .unwrap_or_else(|| panic!("a hit region at ({x}, {y})"))
}

#[test]
fn escape_closes_a_tooltip_a_picker_and_a_window_in_turn_then_opens_the_menu_and_declines() {
    let mut rig = menu_rig(
        &format!("new_window(120, 100) new_bar() {MARKS} {PROBE}"),
        &[
            r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
mark("probe", 630, 420)
"#,
            r#"
eq(count_hits("tooltip_panel_"), 1, "the tooltip is open")
bar.filter_button.on_click()
expect(bar.picker ~= nil and not W.closed and not menu.is_open())
"#,
            r#"
eq(count_hits("tooltip_panel_"), 0, "the first Escape closed the tooltip")
expect(bar.picker ~= nil and not W.closed and not menu.is_open(), "and nothing else")
"#,
            r#"
expect(bar.picker == nil and not W.closed and not menu.is_open(), "the second closed the picker, not the window")
"#,
            r#"
expect(W.closed and not menu.is_open(), "the third closed the window and did not open the menu")
"#,
            r#"
expect(menu.is_open(), "the fourth, with nothing left, opened the menu")
eq(windows.top(), menu.current().window, "the menu is the top window")
"#,
            r#"
expect(not menu.is_open(), "Escape in the menu closed it")
"#,
            r#"
expect(menu.is_open(), "and a further Escape opened it again")
"#,
        ],
    );
    rig.opened();
    let probe = point(&rig, "probe");
    hover(&mut rig, probe);
    for _ in 0..150 {
        frame(&mut rig);
    }
    hover(&mut rig, (700.0, 550.0));
    rig.opened();
    assert!(rig.escape(), "Escape closes the tooltip");
    assert!(sent(&rig).is_empty(), "and the game is not paused for that");
    rig.opened();
    assert!(rig.escape(), "Escape closes the picker");
    rig.opened();
    assert!(rig.escape(), "Escape closes the window");
    assert!(sent(&rig).is_empty());
    rig.opened();
    assert!(
        rig.escape(),
        "with nothing else open Escape opens the menu and declines: the run goes on"
    );
    assert_eq!(sent(&rig), [Control::Pause]);
    rig.opened();
    assert!(rig.escape(), "Escape in the menu closes it and declines");
    assert!(sent(&rig).is_empty(), "closing the menu sends no control");
    rig.opened();
    assert!(
        rig.escape(),
        "a second Escape with nothing open opens it again"
    );
    assert_eq!(sent(&rig), [Control::Pause]);
    rig.opened();
    // The window's own close button still ends the run, with or without the menu.
    assert_eq!(close_request(&mut rig, CloseSource::Button), Answer::Accept);
}

#[test]
fn a_button_close_request_accepts_with_no_menu_open_too() {
    let mut rig = menu_rig(
        "",
        &["expect(not menu.is_open())", "expect(not menu.is_open())"],
    );
    rig.opened();
    assert_eq!(close_request(&mut rig, CloseSource::Button), Answer::Accept);
    rig.opened();
}

#[test]
fn the_menu_is_a_fixed_titled_window_of_five_equal_buttons_centred_in_the_view() {
    let mut rig = menu_rig(
        "",
        &[r#"
menu.open(VIEW)
local win = menu.current().window
eq(win.title, "Menu")
expect(win.fixed and win.draggable == false and win.close_button == nil, "fixed: no close button, not draggable")
eq(win.kind, "window")
local labels = {"return", "settings", "glossary", "save / load", "quit to desktop"}
eq(#menu.LABELS, #labels)
local widest = 0
for index, label in ipairs(labels) do
  eq(menu.LABELS[index], label, "the order")
  widest = math.max(widest, font.layout(label).width)
end
local want_width = (widest + 4 + 16) * P
local previous_bottom
local ox, oy = win:content_origin()
for _, label in ipairs(labels) do
  local b = menu_button(label)
  local w, h = b:size()
  eq(w, want_width, label .. ": every button is the widest label's width + 4 + 16 UI pixels")
  eq(h, 15 * P, label .. ": 15 tall")
  eq(b.x, 0, label .. ": the column's left")
  if previous_bottom ~= nil then
    local gap = label == "quit to desktop" and 9 or 3
    eq(b.y - previous_bottom, gap * P, label .. ": the gap above")
  end
  previous_bottom = b.y + h
  local text_width = font.layout(label).width * P
  expect(math.abs((b.line.x - b.x) - (w - text_width) / 2) <= P, label .. ": the text is centred")
  eq(line_text(b.line), label)
end
eq(win.content_width, want_width, "the column is the window's whole content width")
eq(win.content_height, previous_bottom, "and its whole content height")
local x, y = win:position()
local w, h = win:size()
expect(math.abs(x + w / 2 - 400) <= P and math.abs(y + h / 2 - 300) <= P, "centred in the 800 by 600 view")
-- A second opening comes to the front and makes no second menu.
eq(menu.open(VIEW), win, "the same window")
eq(count_hits("menu_backdrop_"), 1, "one backdrop")
eq(count_hits("window_frame_"), 1, "one window")
"#],
    );
    rig.opened();
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "only the first opening pauses"
    );
}

#[test]
fn the_menu_is_centred_again_and_its_backdrop_covers_the_view_again_when_the_view_is_resized() {
    let mut rig = menu_rig(
        "",
        &[
            r#"
menu.open(VIEW)
local state = menu.current()
local quad = scene.quads[state.backdrop]
eq(quad.tint, theme_lib.colour("body"):sub(1, 7) .. "80", "the colour body at alpha one half")
eq(select(3, quad_at(quad)), 800) eq(select(4, quad_at(quad)), 600)
local _, _, w, h = hit_rect(state.backdrop_key)
eq(w, 800) eq(h, 600)
"#,
            r#"
local state = menu.current()
eq(VIEW.width, 1000) eq(VIEW.height, 700)
local x, y = state.window:position()
local w, h = state.window:size()
expect(math.abs(x + w / 2 - 500) <= P and math.abs(y + h / 2 - 350) <= P, "centred in the 1000 by 700 view")
local quad = scene.quads[state.backdrop]
local qx, qy, qw, qh = quad_at(quad)
eq(qx, 0) eq(qy, 0) eq(qw, 1000) eq(qh, 700)
local hx, hy, hw, hh = hit_rect(state.backdrop_key)
eq(hx, 0) eq(hy, 0) eq(hw, 1000) eq(hh, 700)
"#,
        ],
    );
    rig.opened();
    rig.deliver_fixture_frame_at(1000, 700, 2);
    rig.opened();
}

#[test]
fn the_backdrop_takes_every_press_drag_scroll_and_hover_so_nothing_under_it_reacts() {
    let mut rig = menu_rig(
        "W = windows.new{title = 'under', content_width = 240, content_height = 200, x = 500, y = 300, kind = 'place'} \
         covered = button.new{x = 0, y = 0, text = 'under', group = W.content, owner = W, \
           on_click = function() clicks = (clicks or 0) + 1 end}",
        &[
            r#"
local ox, oy = W:content_origin()
local w, h = covered:size()
mark("covered", ox + w / 2, oy + h / 2)
"#,
            r"
menu.open(VIEW)
mark_buttons()
",
            r#"
eq(clicks or 0, 0, "a press under the backdrop clicked nothing")
expect(not covered.hovered and not covered.pressed, "and gave it no hover or pressed look")
"#,
        ],
    );
    rig.opened();
    let covered = point(&rig, "covered");
    let map_at = rig.map_pointer;
    let moved = |dx: i32| PointerPosition {
        x: map_at.x + dx * 256,
        y: map_at.y,
    };

    // Before the menu opens, the map reacts to this very input and the covered button clicks (the control).
    hover(
        &mut rig,
        (f64::from(map_at.x) / 256.0, f64::from(map_at.y) / 256.0),
    );
    assert!(
        crate::map_app::lock(&rig.map).hover().is_some(),
        "the map hovers a place"
    );
    let start = camera_of(&rig.map);
    rig.feed(&[PointerEvent::Down(map_at), PointerEvent::Move(moved(40))]);
    rig.feed(&[PointerEvent::Up(moved(40))]);
    let panned = camera_of(&rig.map);
    assert_ne!(panned, start, "the map pans by a drag with no menu");
    rig.feed(&[PointerEvent::Scroll {
        position: map_at,
        amount: 256,
        unit: ScrollUnit::Lines,
    }]);
    let zoomed = camera_of(&rig.map);
    assert_ne!(zoomed, panned, "and zooms by a scroll");

    // A drag already captured by the map when the menu opens ends with it.
    rig.feed(&[PointerEvent::Down(map_at), PointerEvent::Move(moved(10))]);
    let dragging = camera_of(&rig.map);
    rig.opened();
    assert_eq!(sent(&rig), [Control::Pause]);
    assert!(
        rig.controls.keys_blocked(),
        "the menu turned the key block on"
    );
    rig.feed(&[PointerEvent::Move(moved(60))]);
    rig.feed(&[PointerEvent::Move(moved(90)), PointerEvent::Up(moved(90))]);
    assert_eq!(
        camera_of(&rig.map),
        dragging,
        "the backdrop alone would not stop it; the drag ended with the opening"
    );

    // The backdrop is the topmost region everywhere but over the menu.
    assert!(key_at(&rig, covered).starts_with("menu_backdrop_"));
    assert!(
        key_at(
            &rig,
            (f64::from(map_at.x) / 256.0, f64::from(map_at.y) / 256.0)
        )
        .starts_with("menu_backdrop_")
    );
    assert!(key_at(&rig, (1.0, 1.0)).starts_with("menu_backdrop_"));
    let return_at = point(&rig, "return");
    assert!(
        key_at(&rig, return_at).starts_with("button_"),
        "the menu's own buttons are above it"
    );

    // A fresh press, drag, scroll and hover over the map reach neither the map nor a window.
    click_at(&mut rig, covered);
    rig.feed(&[PointerEvent::Down(map_at), PointerEvent::Move(moved(40))]);
    rig.feed(&[PointerEvent::Up(moved(40))]);
    rig.feed(&[PointerEvent::Scroll {
        position: map_at,
        amount: 256,
        unit: ScrollUnit::Lines,
    }]);
    hover(
        &mut rig,
        (f64::from(map_at.x) / 256.0, f64::from(map_at.y) / 256.0),
    );
    assert_eq!(
        camera_of(&rig.map),
        dragging,
        "the map neither pans nor zooms"
    );
    assert!(
        crate::map_app::lock(&rig.map).hover().is_none(),
        "and hovers nothing"
    );
    rig.opened();
}

#[test]
fn opening_sends_the_pause_only_control_whatever_the_ui_last_knew_and_return_sends_nothing() {
    let mut rig = menu_rig(
        "",
        &[
            r#"
eq(shown_paused, false, "the UI last heard that the game is running")
menu.open(VIEW)
mark_buttons()
"#,
            "expect(not menu.is_open(), 'return closed the menu')",
            r#"
eq(shown_paused, "player", "the UI last heard that the player paused")
menu.open(VIEW)
mark_buttons()
"#,
            "expect(not menu.is_open())",
        ],
    );
    let running = crate::readout_tests::snapshot(3, 2, None);
    rig.deliver_game_time(&running);
    rig.opened();
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "a game the UI thinks is running pauses by the pause-only control, not by a toggle"
    );
    click(&mut rig, "return");
    rig.opened();
    assert!(
        sent(&rig).is_empty(),
        "return sends no control: it does not resume the game"
    );
    assert!(
        !rig.controls.keys_blocked(),
        "and the key block is off again"
    );

    let paused = crate::readout_tests::snapshot(3, 2, Some(PauseReason::Player));
    rig.deliver_game_time(&paused);
    rig.opened();
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "a game the UI thinks is paused gets the same control, which leaves a paused game as it is, never a toggle"
    );
    click(&mut rig, "return");
    rig.opened();
    assert!(sent(&rig).is_empty());
}

#[test]
fn return_and_escape_close_the_menu_and_its_backdrop_and_leave_the_game_paused() {
    let mut rig = menu_rig(
        "",
        &[
            "menu.open(VIEW) mark_buttons() assert(count_hits('menu_backdrop_') == 1)",
            r#"
expect(not menu.is_open(), "return closed the menu")
eq(count_hits("menu_backdrop_"), 0, "and its backdrop")
eq(count_hits("window_frame_"), 0, "and its window")
menu.open(VIEW)
"#,
            r#"
expect(not menu.is_open(), "Escape closed it")
eq(count_hits("menu_backdrop_"), 0)
"#,
        ],
    );
    rig.opened();
    assert_eq!(sent(&rig), [Control::Pause]);
    assert!(rig.controls.keys_blocked());
    click(&mut rig, "return");
    assert!(
        !rig.controls.keys_blocked(),
        "return turns the key block off"
    );
    assert!(
        sent(&rig).is_empty(),
        "and sends nothing: the game stays paused"
    );
    // The step closes nothing: it checks, and opens the menu again, for Escape.
    rig.opened();
    assert_eq!(sent(&rig), [Control::Pause]);
    assert!(rig.controls.keys_blocked());
    assert!(
        rig.escape(),
        "Escape with the menu open closes it and declines"
    );
    assert!(
        !rig.controls.keys_blocked(),
        "Escape turns the block off too"
    );
    rig.opened();
    assert!(sent(&rig).is_empty(), "and sends no control either");
}

#[test]
fn quit_to_desktop_ends_the_run_at_once_with_no_question() {
    let mut rig = menu_rig(
        "",
        &[
            "menu.open(VIEW) mark_buttons() eq(quit_calls, 0)",
            "eq(quit_calls, 1, 'one window.close') expect(menu.is_open(), 'asked nothing, closed nothing else')",
        ],
    );
    rig.opened();
    click(&mut rig, "quit to desktop");
    rig.opened();
    assert_eq!(sent(&rig), [Control::Pause], "no save and no other control");
}

#[test]
fn settings_and_glossary_are_live_while_save_load_stays_greyed() {
    let mut rig = menu_rig(
        "",
        &[
            r#"
menu.open(VIEW)
mark_buttons()
local settings = menu_button("settings")
expect(not settings.inactive and not dim_look(settings), "settings is live")
local glossary_button = menu_button("glossary")
expect(not glossary_button.inactive and not dim_look(glossary_button), "glossary is live")
for _, label in ipairs({"save / load"}) do
  expect(dim_look(menu_button(label)), label .. ": text dim, outline track, the body inside")
  expect(menu_button(label).inactive, label .. " is inactive")
end
local live = menu_button("return")
expect(not dim_look(live), "return is a live button")
eq(live.frame.line, theme_lib.kind("window").outline)
MENU_LOOK = {}
for _, label in ipairs(menu.LABELS) do
  local b = menu_button(label)
  MENU_LOOK[label] = {b.frame.fill, b.frame.line, b.line.test_runs[1].colour}
end
"#,
            r#"
for _, label in ipairs({"save / load"}) do
  local b = menu_button(label)
  expect(dim_look(b), label .. ": no hover or pressed look")
  expect(not b.pressed, label .. ": never pressed")
  eq(b.frame.fill .. b.frame.line .. b.line.test_runs[1].colour, MENU_LOOK[label][1] .. MENU_LOOK[label][2] .. MENU_LOOK[label][3])
end
expect(menu.is_open(), "a press on a greyed button does nothing: the menu is still there")
eq(quit_calls, 0)
local live = menu_button("return")
expect(live.hovered, "the live button shows its hover (the pointer is on it)")
eq(live.frame.line, theme_lib.kind("window").hot, "return's hover look")
"#,
        ],
    );
    rig.opened();
    let at = point(&rig, "save / load");
    hover(&mut rig, at);
    rig.feed(&[PointerEvent::Down(points(at.0, at.1))]);
    frame(&mut rig);
    rig.feed(&[PointerEvent::Up(points(at.0, at.1))]);
    frame(&mut rig);
    let at = point(&rig, "return");
    hover(&mut rig, at);
    rig.opened();
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "nothing but the opening's pause"
    );
}

#[test]
fn an_inactive_button_can_be_made_live_again_and_a_wide_button_centres_its_text() {
    let mut rig = menu_rig(
        r#"
new_window(120, 60)
presses = 0
live = button.new{x = 0, y = 0, text = "go", group = W.content, owner = W, inactive = true,
  on_click = function() presses = presses + 1 end}
wide = button.new{x = 0, y = 40, text = "go", width = 60 * P, group = W.content, owner = W}
narrow = button.new{x = 0, y = 80, text = "a long label", width = 4 * P, group = W.content, owner = W}
"#,
        &[
            r#"
local natural = button.measure("go")
eq(select(1, wide:size()), 60 * P, "the width option")
eq(select(2, wide:size()), select(2, live:size()), "the height is the text's")
expect(select(1, narrow:size()) == button.measure("a long label"), "never narrower than its text needs")
expect(select(1, live:size()) == natural, "no width option: the text's own")
local text_width = font.layout("go").width * P
expect(math.abs((wide.line.x - wide.x) - (60 * P - text_width) / 2) <= P, "the text is centred in the wide button")
expect(dim_look(live), "starts greyed")
local ox, oy = W:content_origin()
local w, h = live:size()
mark("live", ox + w / 2, oy + h / 2)
"#,
            r#"
eq(presses, 0, "a greyed button's press does nothing")
expect(not live.hovered or dim_look(live), "and its look stays")
live:set_inactive(false)
expect(not dim_look(live), "made live again")
"#,
            r#"
eq(presses, 1, "a live button clicks again")
live:set_inactive(true)
expect(dim_look(live), "and greys again")
"#,
            r#"
eq(presses, 1, "greyed again: no further click")
"#,
        ],
    );
    rig.opened();
    click(&mut rig, "live");
    rig.opened();
    click(&mut rig, "live");
    rig.opened();
    click(&mut rig, "live");
    rig.opened();
}

const SETTINGS_HOST: &str = r#"
fixture = {
  tooltips = {delay = 0.3, lock = "middle_mouse", lock_timer = 1.5},
  map = {pan_pixels_per_frame = 4, zoom_steps_per_second = 20, scroll_points_per_step = 24},
  calls = {},
  bindings = {
    {name="close", bindings={"Escape"}},
    {name="time_pause", bindings={"Space"}},
    {name="time_speed_1", bindings={"Digit1"}},
    {name="time_speed_2", bindings={"Digit2"}},
    {name="time_speed_3", bindings={"Digit3"}},
    {name="time_speed_4", bindings={"Digit4"}},
    {name="time_speed_5", bindings={"Digit5"}},
    {name="time_faster", bindings={"BracketRight"}},
    {name="time_slower", bindings={"BracketLeft"}},
    {name="game_quicksave", bindings={"F5"}},
    {name="game_quickload", bindings={"F9"}},
    {name="map_pan_left", bindings={"ArrowLeft"}},
    {name="map_pan_right", bindings={"ArrowRight"}},
    {name="map_pan_up", bindings={"ArrowUp"}},
    {name="map_pan_down", bindings={"ArrowDown"}},
    {name="map_zoom_in", bindings={"Equal", "Shift+Equal"}},
    {name="map_zoom_out", bindings={"Minus"}},
  }
}
settings = {
  tooltips = function() return fixture.tooltips end,
  map = function() return fixture.map end,
  bindings = function() return fixture.bindings end,
  set = function(name, value)
    fixture.calls[#fixture.calls + 1] = {"set", name, value}
    if name == "ui.tooltips.delay" then fixture.tooltips.delay = value
    elseif name == "ui.tooltips.lock" then fixture.tooltips.lock = value
    elseif name == "ui.tooltips.lock_timer" then fixture.tooltips.lock_timer = value
    elseif name == "map.pan_pixels_per_frame" then fixture.map.pan_pixels_per_frame = value
    elseif name == "map.zoom_steps_per_second" then fixture.map.zoom_steps_per_second = value
    elseif name == "map.scroll_points_per_step" then fixture.map.scroll_points_per_step = value end
  end,
  reset = function(tab)
    fixture.calls[#fixture.calls + 1] = {"reset", tab}
    if tab == "tooltips" then fixture.tooltips = {delay=0.3, lock="middle_mouse", lock_timer=1.5}
    else fixture.map = {pan_pixels_per_frame=4, zoom_steps_per_second=20, scroll_points_per_step=24} end
  end,
}
function mark_settings()
  local s = require("lib.settings_window").current()
  if s == nil then return end
  local ox, oy = s.window:content_origin()
  for name, b in pairs(s.tabs) do
    local x, y, w, h = hit_rect(b.key)
    mark("settings_tab_" .. name, x + w / 2, y + h / 2)
  end
  for name, control in pairs(s.controls) do
    if control.key ~= nil then
      local x, y, w, h = hit_rect(control.key)
      mark("settings_control_" .. name, x + w / 2, y + h / 2)
      if control.knob ~= nil and control.track ~= nil then
        local kx, ky, kw, kh = quad_at(scene.quads[control.knob])
        local tx, ty, tw, th = quad_at(scene.quads[control.track])
        mark("settings_slider_knob_" .. name, kx + kw / 2, ky + kh / 2)
        mark("settings_slider_mid_" .. name, tx + tw / 2, ty + th / 2)
        mark("settings_slider_right_" .. name, tx + tw - 1, ty + th / 2)
      end
    elseif type(control) == "table" then
      for index, item in ipairs(control) do
        local x, y, w, h = hit_rect(item.key)
        mark("settings_control_" .. name .. "_" .. index, x + w / 2, y + h / 2)
      end
    end
  end
  if s.window.close_button ~= nil then
    local b = s.window.close_button
    local _, y = s.window:position()
    local x, h
    x = select(1, s.window:position())
    local w
    w, h = b:size()
    mark("settings_close", x + b.x + w / 2, y + b.y + h / 2)
  end
  mark("settings_content_origin", ox, oy)
end
"#;

fn settings_rig(steps: &[&str]) -> Rig {
    menu_rig(
        &format!("{SETTINGS_HOST}\nmenu.open(VIEW) mark_buttons()"),
        steps,
    )
}

#[test]
fn settings_opens_from_the_menu_as_a_centred_resizable_window_with_the_three_side_tabs() {
    let mut rig = settings_rig(&[
        r#"
expect(not menu_button("settings").inactive, "settings is a live menu action")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s ~= nil, "the menu's settings button opens the settings window")
local win = s.window
eq(win.title, "Settings")
eq(win.kind, "window")
expect(not win.fixed and not win.draggable and win.close_button ~= nil, "a close x, not draggable")
eq(s.selected, "general", "opens on general")
eq(#s.tab_order, 3)
eq(s.tab_order[1], "general") eq(s.tab_order[2], "map") eq(s.tab_order[3], "input")
expect(s.tabs.graphics == nil and s.tabs.gameplay == nil, "no graphics or gameplay row in slice A")
local widest = math.max(font.layout("general").width, font.layout("graphics").width,
  font.layout("map").width, font.layout("input").width)
eq(s.tab_width, (widest + 12) * P, "the reserved column includes graphics")
eq(s.tabs.general.width, s.tab_width - 3 * P, "the tab fill stops three UI pixels before the divider")
eq(s.tabs.map.y - s.tabs.general.y, 13 * P, "tab rows are thirteen UI pixels tall")
expect(s.tabs.general.selected and s.tabs.general.frame.fill == theme_lib.kind("window").hot)
local x, y = win:position()
local w, h = win:size()
expect(math.abs(x + w / 2 - 400) <= P and math.abs(y + h / 2 - 300) <= P, "centred")
local _, _, _, divider_height = quad_at(scene.quads[s.divider])
eq(divider_height, s.content_height - 6 * P, "the divider ends six UI pixels above the content bottom")
local original = win
menu_button("settings").on_click()
eq(require("lib.settings_window").current().window, original, "a second opening raises the same window")
eq(count_hits("window_frame_"), 2, "there is one settings window beside the menu")
mark_settings()
"#,
        r#"
expect(menu.is_open(), "the menu stays open behind settings")
local s = require("lib.settings_window").current()
expect(s ~= nil and s.selected == "general")
"#,
        r#"
local s = require("lib.settings_window").current()
local a, b = s.window:position()
local w, h = s.window:size()
expect(math.abs(a + w / 2 - 500) <= P and math.abs(b + h / 2 - 350) <= P, "settings recentres after resize")
"#,
    ]);
    rig.opened();
    let settings = point(&rig, "settings");
    click_at(&mut rig, settings);
    rig.opened();
    let center_key = key_at(&rig, (400.0, 300.0));
    assert!(
        center_key.starts_with("window_frame_"),
        "settings is above the menu"
    );
    let _ = point(&rig, "settings_content_origin");
    rig.deliver_fixture_frame_at(1000, 700, 2);
    rig.opened();
}

#[test]
fn settings_general_sliders_choices_and_defaults_use_the_settings_calls() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r#"
local s = require("lib.settings_window").current()
expect(s ~= nil and s.selected == "general")
expect(not s.controls["lock timer"].enabled, "lock timer starts disabled")
eq(s.controls["show delay"].minimum, 0) eq(s.controls["show delay"].maximum, 2) eq(s.controls["show delay"].step, 0.1)
eq(s.controls["lock timer"].minimum, 0.3) eq(s.controls["lock timer"].maximum, 3) eq(s.controls["lock timer"].step, 0.1)
eq(#s.controls.lock, 3)
eq(s.controls.lock[3].y - s.controls.lock[1].y, 18 * P, "the choice row wraps to a second line")
expect(s.controls.lock[3].x == s.controls.lock[1].x, "wrapped rows start at the same left edge")
expect(s.controls.lock[2].selected, "the effective lock choice is selected")
eq(s.controls["lock timer"].value_label.line.colour, theme_lib.colour("dim"), "disabled timer value is dim")
eq(scene.quads[s.controls["lock timer"].track].tint, theme_lib.colour("dim"), "disabled timer track is dim")
for _, line in ipairs(lines_in(s.page.group)) do
  expect(line.x + line.width * P <= s.page.room + P, "general row stays within the content right edge")
end
mark_settings()
"#,
        r#"
eq(#fixture.calls, 0, "a disabled slider does not respond")
expect(fixture.tooltips.lock == "middle_mouse")
"#,
        r#"
expect(fixture.tooltips.lock == "timer", "choice writes through settings.set")
local s = require("lib.settings_window").current()
expect(s.controls["lock timer"].enabled, "timer choice enables its slider")
eq(fixture.calls[#fixture.calls][1], "set")
eq(fixture.calls[#fixture.calls][2], "ui.tooltips.lock")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls["lock timer"].value, 3, "dragging the knob reaches the upper bound")
eq(fixture.calls[#fixture.calls][2], "ui.tooltips.lock_timer")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
local value = s.controls["show delay"].value
expect(value >= 0 and value <= 2 and math.abs(value * 10 - math.floor(value * 10 + 0.5)) < 1e-8,
  "show delay is snapped to a tenth")
eq(fixture.calls[#fixture.calls][2], "ui.tooltips.delay")
eq(line_text(s.controls["show delay"].value_label.line), string.format("%.1f s", value))
mark_settings()
"#,
        r#"
expect(#fixture.calls >= 3)
eq(fixture.calls[#fixture.calls][1], "reset")
eq(fixture.calls[#fixture.calls][2], "tooltips")
eq(fixture.tooltips.delay, 0.3) eq(fixture.tooltips.lock, "middle_mouse") eq(fixture.tooltips.lock_timer, 1.5)
eq(fixture.map.pan_pixels_per_frame, 4, "reset affects only the current tab")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_control_lock timer");
    rig.opened();
    click_marked(&mut rig, "settings_control_lock_3");
    rig.opened();
    drag_marked(
        &mut rig,
        "settings_slider_knob_lock timer",
        "settings_slider_right_lock timer",
    );
    rig.opened();
    click_marked(&mut rig, "settings_control_show delay");
    rig.opened();
    click_marked(&mut rig, "settings_control_defaults");
    rig.opened();
}

#[test]
fn settings_general_slider_stays_live_during_a_drag_across_frame_updates() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r#"
mark_settings()
dragged_control = require("lib.settings_window").current().controls["show delay"]
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["show delay"] == dragged_control, "the slider remains registered after the press frame")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["show delay"] == dragged_control, "the slider remains registered after the middle move")
eq(s.controls["show delay"].value, 1.0, "the middle move updates the value")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["show delay"] == dragged_control, "the slider remains registered after the final move")
eq(s.controls["show delay"].value, 2.0)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["show delay"] == dragged_control, "the slider remains registered through release")
eq(s.controls["show delay"].value, 2.0)
eq(line_text(s.controls["show delay"].value_label.line), "2.0 s", "the live value is shown")
eq(fixture.calls[#fixture.calls][1], "set")
eq(fixture.calls[#fixture.calls][2], "ui.tooltips.delay")
eq(fixture.calls[#fixture.calls][3], 2.0, "the last snapped value reaches settings.set")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    let from = point(&rig, "settings_slider_knob_show delay");
    let middle = point(&rig, "settings_slider_mid_show delay");
    let right = point(&rig, "settings_slider_right_show delay");
    rig.feed(&[PointerEvent::Down(points(from.0, from.1))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Move(points(middle.0, middle.1))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Move(points(right.0, right.1))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Up(points(right.0, right.1))]);
    frame(&mut rig);
    rig.opened();
}

#[test]
fn settings_map_slider_stays_live_during_a_drag_across_frame_updates() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r"mark_settings()",
        r#"
mark_settings()
dragged_control = require("lib.settings_window").current().controls["pan speed"]
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["pan speed"] == dragged_control, "the slider remains registered after the press frame")
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["pan speed"] == dragged_control, "the slider remains registered after the middle move")
expect(s.controls["pan speed"].value > 4 and s.controls["pan speed"].value < 32)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["pan speed"] == dragged_control, "the slider remains registered after the final move")
eq(s.controls["pan speed"].value, 32)
"#,
        r#"
local s = require("lib.settings_window").current()
expect(s.controls["pan speed"] == dragged_control, "the slider remains registered through release")
eq(s.controls["pan speed"].value, 32)
eq(line_text(s.controls["pan speed"].value_label.line), "32", "the live value is shown")
eq(fixture.calls[#fixture.calls][1], "set")
eq(fixture.calls[#fixture.calls][2], "map.pan_pixels_per_frame")
eq(fixture.calls[#fixture.calls][3], 32, "the last snapped value reaches settings.set")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_map");
    rig.opened();
    let from = point(&rig, "settings_slider_knob_pan speed");
    let middle = point(&rig, "settings_slider_mid_pan speed");
    let right = point(&rig, "settings_slider_right_pan speed");
    rig.feed(&[PointerEvent::Down(points(from.0, from.1))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Move(points(middle.0, middle.1))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Move(points(right.0, right.1))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Up(points(right.0, right.1))]);
    frame(&mut rig);
    rig.opened();
}

#[test]
fn shared_button_flow_keeps_fitting_rows_on_one_line_and_wraps_with_exact_gaps() {
    let mut rig = settings_rig(&[r#"
local button = require("lib.button")
local group = render.create_group()
local width = button.measure("same")
local fitted, fitted_height = button.flow{
  x = 20 * P, y = 10 * P, room = 3 * width + 2 * 3 * P,
  items = {"same", "same", "same"}, group = group,
}
eq(#fitted, 3)
eq(fitted_height, 15 * P, "one line has the measured button height")
for index = 2, #fitted do
  eq(fitted[index].y, fitted[1].y, "a row that fits stays on one line")
  eq(fitted[index].x - fitted[index - 1].x - fitted[index - 1].width, 3 * P, "buttons have a three pixel gap")
end
local wrapped, wrapped_height = button.flow{
  x = 20 * P, y = 80 * P, room = 2 * width + 3 * P,
  items = {"same", "same", "same", "same", "same"}, group = group,
}
eq(#wrapped, 5)
eq(wrapped_height, 51 * P, "three lines report their full height")
for index = 2, #wrapped do
  if index == 3 or index == 5 then
    eq(wrapped[index].x, wrapped[1].x, "every wrapped line begins at the same left edge")
    eq(wrapped[index].y - wrapped[index - 1].y, 18 * P, "wrapped lines are eighteen pixels apart")
  else
    eq(wrapped[index].y, wrapped[index - 1].y)
    eq(wrapped[index].x - wrapped[index - 1].x - wrapped[index - 1].width, 3 * P, "wrapped buttons retain the three pixel gap")
  end
end
"#]);
    rig.opened();
}

#[test]
fn settings_map_sliders_show_effective_values_and_reset_only_map() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "map")
eq(s.controls["pan speed"].minimum, 1) eq(s.controls["pan speed"].maximum, 32)
eq(s.controls["zoom speed"].minimum, 1) eq(s.controls["zoom speed"].maximum, 60)
eq(s.controls["scroll step"].minimum, 1) eq(s.controls["scroll step"].maximum, 256)
eq(s.controls["pan speed"].step, 1) eq(s.controls["zoom speed"].step, 1) eq(s.controls["scroll step"].step, 1)
eq(line_text(s.controls["pan speed"].value_label.line), "4")
eq(line_text(s.controls["zoom speed"].value_label.line), "20")
eq(line_text(s.controls["scroll step"].value_label.line), "24 points")
for _, line in ipairs(lines_in(s.page.group)) do
  expect(line.x + line.width * P <= s.page.room + P, "map row stays within the content right edge")
end
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "map")
local value = s.controls["pan speed"].value
expect(value >= 1 and value <= 32 and value == math.floor(value), "pan speed snaps to an integer")
eq(fixture.calls[#fixture.calls][1], "set")
eq(fixture.calls[#fixture.calls][2], "map.pan_pixels_per_frame")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls["zoom speed"].value, 60, "dragging the knob reaches the zoom upper bound")
eq(fixture.calls[#fixture.calls][2], "map.zoom_steps_per_second")
mark_settings()
"#,
        r#"
local s = require("lib.settings_window").current()
eq(s.controls["scroll step"].value, 256, "dragging the knob reaches the scroll upper bound")
eq(fixture.calls[#fixture.calls][2], "map.scroll_points_per_step")
mark_settings()
"#,
        r#"
eq(fixture.calls[#fixture.calls][1], "reset")
eq(fixture.calls[#fixture.calls][2], "map")
local s = require("lib.settings_window").current()
eq(s.controls["pan speed"].value, 4)
eq(s.controls["zoom speed"].value, 20)
eq(s.controls["scroll step"].value, 24)
eq(fixture.tooltips.delay, 0.3, "map reset leaves general values alone")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_map");
    rig.opened();
    click_marked(&mut rig, "settings_control_pan speed");
    rig.opened();
    drag_marked(
        &mut rig,
        "settings_slider_knob_zoom speed",
        "settings_slider_right_zoom speed",
    );
    rig.opened();
    drag_marked(
        &mut rig,
        "settings_slider_knob_scroll step",
        "settings_slider_right_scroll step",
    );
    rig.opened();
    click_marked(&mut rig, "settings_control_defaults");
    rig.opened();
}

#[test]
fn settings_input_rows_format_bindings_abbreviate_arrows_and_clip_long_names() {
    let mut rig = settings_rig(&[
        r#"
for _, action in ipairs(fixture.bindings) do
  if action.name == "game_quicksave" then action.bindings = {"Control+VeryLongPhysicalKeyName"} end
  if action.name == "game_quickload" then action.bindings = {} end
end
"#,
        r"mark_settings()",
        r#"
local s = require("lib.settings_window").current()
eq(s.selected, "input")
expect(s.controls.defaults == nil, "input has no defaults button")
local found = {}
local all = {}
for _, line in ipairs(lines_in(s.page.group)) do
  local shown = line_text(line)
  found[shown] = true
  all[#all + 1] = shown
  expect(line.x + line.width * P <= s.page.room + P, "row stays within the content right edge")
end
for _, value in ipairs({"pause", "Space", "speed 1 to 5", "1  2  3  4  5", "faster, slower", "]  [",
  "quicksave", "Control+", "quickload", "move the map", "arrows", "zoom in, out", "=  Shift+Equal  -", "close, menu", "Escape",
  "changing keys comes later"}) do
  local matched = false
  for _, shown in ipairs(all) do if shown:find(value, 1, true) then matched = true end end
  expect(matched, "input shows " .. value)
end
expect(not table.concat(all, " "):find("ArrowLeft", 1, true), "the exact four default arrows collapse to arrows")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_input");
    rig.opened();
}

#[test]
fn settings_close_x_returns_to_the_open_menu_without_releasing_its_key_block() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r#"
expect(require("lib.settings_window").current() ~= nil)
mark_settings()
"#,
        r#"
expect(menu.is_open(), "the settings x leaves the menu open")
expect(require("lib.settings_window").current() == nil)
eq(count_hits("menu_backdrop_"), 1)
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_close");
    rig.opened();
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "closing settings adds no game control"
    );
    assert!(rig.controls.keys_blocked());
}

#[test]
fn closing_the_menu_closes_its_settings_child_and_releases_the_key_block() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r#"
local settings = require("lib.settings_window").current()
expect(settings ~= nil and menu.is_open())
menu.close()
expect(not menu.is_open(), "the parent menu closed")
expect(require("lib.settings_window").current() == nil, "closing the parent closes settings")
eq(count_hits("window_frame_"), 0)
eq(count_hits("menu_backdrop_"), 0)
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    assert!(!rig.controls.keys_blocked());
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "closing both windows sends no additional control"
    );
}

#[test]
fn late_windows_stay_under_the_menu_backdrop_and_escape_closes_modal_windows_first() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r#"
late_created_clicks, late_raised_clicks = 0, 0
late_created = windows.new{title="late created", content_width=90*P, content_height=50*P, x=20*P, y=20*P}
button.new{x=0,y=0,text="created",group=late_created.content,owner=late_created,on_click=function() late_created_clicks=late_created_clicks+1 end}
late_raised = windows.new{title="late raised", content_width=90*P, content_height=50*P, x=300*P, y=20*P}
button.new{x=0,y=0,text="raised",group=late_raised.content,owner=late_raised,on_click=function() late_raised_clicks=late_raised_clicks+1 end}
late_raised:raise()
expect(require("lib.settings_window").current() ~= nil and windows.top() == require("lib.settings_window").current().window,
  "the modal settings window remains the top eligible window")
mark("late_created", 30*P, 50*P)
mark("late_raised", 310*P, 50*P)
"#,
        r#"
eq(late_created_clicks, 0) eq(late_raised_clicks, 0)
expect(require("lib.settings_window").current() == nil, "Escape closes settings first")
expect(menu.is_open() and not late_created.closed and not late_raised.closed)
"#,
        r#"
expect(not menu.is_open(), "the next Escape closes the menu")
expect(not late_created.closed and not late_raised.closed, "covered windows remain open")
"#,
        r#"
eq(late_created_clicks + late_raised_clicks, 1, "late windows take input again after the menu closes")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    let made = point(&rig, "late_created");
    let raised = point(&rig, "late_raised");
    assert!(key_at(&rig, made).starts_with("menu_backdrop_"));
    click_at(&mut rig, made);
    assert!(key_at(&rig, raised).starts_with("menu_backdrop_"));
    click_at(&mut rig, raised);
    assert!(rig.escape());
    rig.opened();
    assert_eq!(
        sent(&rig),
        [Control::Pause],
        "Escape from settings sends no additional game control"
    );
    assert!(
        rig.controls.keys_blocked(),
        "Escape leaves the menu key block on"
    );
    assert!(rig.escape());
    rig.opened();
    click_marked(&mut rig, "late_created");
    rig.opened();
}

#[test]
fn a_window_that_predates_the_menu_stays_under_its_backdrop_when_raised() {
    let mut rig = menu_rig(
        &format!(
            r#"
{SETTINGS_HOST}
preexisting_clicks = 0
preexisting = windows.new{{title="preexisting", content_width=90*P, content_height=50*P, x=20*P, y=20*P}}
preexisting_button = require("lib.button").new{{x=0,y=0,text="preexisting",group=preexisting.content,owner=preexisting,
  on_click=function() preexisting_clicks=preexisting_clicks+1 end}}
menu.open(VIEW)
mark_buttons()
"#
        ),
        &[
            r#"
expect(menu.is_open())
preexisting:raise()
menu_button("settings").on_click()
local x, y, w, h = hit_rect(preexisting_button.key)
mark("preexisting_button", x + w / 2, y + h / 2)
"#,
            r#"
eq(preexisting_clicks, 0, "the backdrop blocks a pre-existing window that was raised")
"#,
            r#"
expect(menu.is_open() and require("lib.settings_window").current() == nil,
  "Escape closes settings but leaves the menu open")
expect(not preexisting.closed, "the older window remains open")
"#,
            r#"
expect(not menu.is_open(), "the next Escape closes the menu")
expect(not preexisting.closed, "the older window remains open")
"#,
            r#"eq(preexisting_clicks, 1, "the older window receives input again after the menu closes")"#,
        ],
    );
    rig.opened();
    let button = point(&rig, "preexisting_button");
    assert!(key_at(&rig, button).starts_with("menu_backdrop_"));
    click_at(&mut rig, button);
    rig.opened();
    assert!(rig.escape());
    rig.opened();
    assert!(rig.escape());
    rig.opened();
    click_at(&mut rig, button);
    rig.opened();
}

/// The Lua both slider-geometry tests share: the labels of the page by their text, and one slider's checks. A row at `y` has its label
/// and value text at `y + 2`, its knob (3 by 7) from `y + 4` and its track (2 high) at `y + 7` (the mock-up's `slider_row`), so a slider
/// is checked against its own label's place; the press and drag region must cover the drawn knob and track.
const SLIDER_GEOMETRY: &str = r#"
function page_labels(s)
  local labels = {}
  for _, line in ipairs(lines_in(s.page.group)) do labels[line_text(line)] = line end
  return labels
end
function check_slider(s, labels, name)
  local c = s.controls[name]
  expect(labels[name] ~= nil, name .. " has its label")
  local row_label_y = labels[name].y
  local knob, track, fill = scene.quads[c.knob], scene.quads[c.track], scene.quads[c.fill]
  eq(knob.y, row_label_y + 2 * P, name .. ": the knob's top is the row's y + 4")
  eq(knob.w, 3 * P, name .. ": the knob is 3 wide") eq(knob.h, 7 * P, name .. ": and 7 tall")
  eq(track.y, row_label_y + 5 * P, name .. ": the track's top is the row's y + 7")
  eq(track.h, 2 * P, name .. ": the track is 2 high")
  eq(fill.y, track.y, name .. ": the fill is on the track") eq(fill.h, 2 * P)
  local hx, hy, hw, hh = hit_rect(c.key)
  for _, part in ipairs({{"knob", knob}, {"track", track}}) do
    local x, y, w, h = quad_at(part[2])
    expect(hx <= x and hy <= y and hx + hw >= x + w and hy + hh >= y + h, name .. ": the press region covers the drawn " .. part[1])
  end
  return row_label_y
end
"#;

#[test]
fn the_general_tab_labels_show_delay_and_places_its_sliders_as_drawn() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        &format!(
            r#"
{SLIDER_GEOMETRY}
local s = require("lib.settings_window").current()
eq(s.selected, "general")
local labels = page_labels(s)
local show = labels["show delay"]
expect(show ~= nil, "the first row has its label")
eq(show.x, 0, "at the content's left") eq(show.y, 16 * P, "at the row's y + 2")
eq(show.colour, theme_lib.colour("text"), "in the text colour")
expect(show.width <= 84, "within the 84-pixel label width")
expect(labels["lock"] ~= nil and labels["lock timer"] ~= nil, "the other two rows keep theirs")
eq(labels["lock"].x, 0) eq(labels["lock"].y, 36 * P)
eq(labels["lock timer"].x, 0)
local delay_y = check_slider(s, labels, "show delay")
eq(scene.quads[s.controls["show delay"].knob].y, 18 * P, "the first row is at 14: its knob from 18")
eq(scene.quads[s.controls["show delay"].track].y, 21 * P, "and its track at 21")
check_slider(s, labels, "lock timer")
mark_settings()
"#
        ),
        r#"
local s = require("lib.settings_window").current()
eq(s.controls["show delay"].value, 1.0, "a press on the drawn track sets the value")
eq(fixture.calls[#fixture.calls][2], "ui.tooltips.delay")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_slider_mid_show delay");
    rig.opened();
}

#[test]
fn the_map_tab_places_its_sliders_as_drawn() {
    let mut rig = settings_rig(&[
        r#"expect(not menu_button("settings").inactive)"#,
        r"mark_settings()",
        &format!(
            r#"
{SLIDER_GEOMETRY}
local s = require("lib.settings_window").current()
eq(s.selected, "map")
local labels = page_labels(s)
for index, name in ipairs({{"pan speed", "zoom speed", "scroll step"}}) do
  local row_y = check_slider(s, labels, name) - 2 * P
  eq(row_y, (index - 1) * 20 * P, name .. ": rows are 20 apart from 0")
end
mark_settings()
"#
        ),
        r#"
local s = require("lib.settings_window").current()
eq(s.controls["pan speed"].value, 17, "a press on the drawn track sets the value")
eq(fixture.calls[#fixture.calls][2], "map.pan_pixels_per_frame")
"#,
    ]);
    rig.opened();
    click_marked(&mut rig, "settings");
    rig.opened();
    click_marked(&mut rig, "settings_tab_map");
    rig.opened();
    click_marked(&mut rig, "settings_slider_mid_pan speed");
    rig.opened();
}
