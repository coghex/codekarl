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
const MENU: &str = r#"
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

fn menu_rig(body: &str, steps: &[&str]) -> Rig {
    rig(&format!("{MENU}\n{body}"), steps)
}

/// The simulation's controls the script has sent since the last call.
fn sent(rig: &Rig) -> Vec<Control> {
    rig.control_receiver
        .try_iter()
        .map(|message| match message {
            Message::Control(control) => control,
            other => panic!("only controls are sent: {other:?}"),
        })
        .collect()
}

fn close_request(rig: &mut Rig, source: CloseSource) -> Answer {
    rig.link
        .post(UiEvent::CloseRequested { source })
        .expect("the close request is accepted");
    answer(&mut rig.link).answer
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
fn settings_glossary_and_save_load_are_greyed_and_do_nothing_and_return_is_live() {
    let mut rig = menu_rig(
        "",
        &[
            r#"
menu.open(VIEW)
mark_buttons()
for _, label in ipairs({"settings", "glossary", "save / load"}) do
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
for _, label in ipairs({"settings", "glossary", "save / load"}) do
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
    for label in ["settings", "glossary", "save / load"] {
        let at = point(&rig, label);
        hover(&mut rig, at);
        rig.feed(&[PointerEvent::Down(points(at.0, at.1))]);
        frame(&mut rig);
        rig.feed(&[PointerEvent::Up(points(at.0, at.1))]);
        frame(&mut rig);
    }
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
