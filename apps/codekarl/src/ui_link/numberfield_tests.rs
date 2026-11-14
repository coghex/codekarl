//! The number field (CK-306, `scripts/ui/lib/numberfield.lua`) in a real UI VM, in the script test rig, with no window and no GPU.
//!
//! The shipped library runs over the real `render` API and the shipped font, theme and art; the scene shadow of `listbar_tests`'s
//! prelude records every quad and text line. Typing is real: the loop's mapper gathers typed entries, the tick carries them to the link,
//! and the script's frame hook runs `typing.dispatch` before `pointer.dispatch`, as `main.lua` does. Pointer cases press real regions.
//! A test reads the field's state from a line the script notes (`state ...`). Nothing sleeps.

use ck_input::{Bindings, Key, Mapper};
use ck_window::{PointerButton, PointerEvent, TypedEntry};

use super::group_tests::Rig;
use super::listbar_tests::{PROBE, click, click_at, frame, hover, notes, point, rig};
use super::widget_witness_tests::points;
use crate::game::controls::Control;
use crate::game::core::Message;

/// Two number fields (`A`, 1920 in 640 to 7680; `B`, 480 in 480 to 4320, three digits so that a digit has room), a button that counts its
/// clicks, the spy on `window.set_text_input`, and the frame hook of `main.lua` (typing, pointer, tooltips).
const BODY: &str = r#"
numberfield = require("lib.numberfield")
typing = require("lib.typing")
menu = require("lib.menu")
VIEW = {width = 800, height = 600}
local real_set_text_input = window.set_text_input
window.set_text_input = function(on)
  local generation = real_set_text_input(on)
  note("input " .. tostring(on) .. " " .. generation)
  return generation
end
new_window(200, 60, "window")
fields, applied, clicks = {}, {}, 0
-- What a caller's `on_apply` does besides noting the number; a test replaces it (to close the window, say).
function after_apply() end
function add(name, x, value, least, greatest, digits)
  fields[name] = numberfield.new{x = x, y = 0, value = value, least = least, greatest = greatest, digits = digits,
    group = W.content, owner = W, on_apply = function(v) applied[#applied + 1] = name .. "=" .. v after_apply(name) end}
end
add("A", 0, 1920, 640, 7680)
add("B", 100, 480, 480, 4320)
shove = button.new{x = 0, y = 40, text = "elsewhere", group = W.content, owner = W, on_click = function() clicks = clicks + 1 end}
function shown(name) return line_text(fields[name].line) end
function marks()
  local ox, oy = content_origin()
  for name, f in pairs(fields) do
    local w, h = f:size()
    mark(name, ox + f.x + w / 2, oy + f.y + h / 2)
  end
  local w, h = shove:size()
  mark("button", ox + shove.x + w / 2, oy + shove.y + h / 2)
  mark("map", 700, 500)
end
function report()
  local who = "none"
  for name, f in pairs(fields) do if f:editing() then who = name end end
  note(string.format("state applied=%s editing=%s A=%s B=%s clicks=%d", table.concat(applied, ","), who, shown("A"), shown("B"), clicks))
end
function on_frame() end
frame.on_update(function(update)
  typing.dispatch(update)
  pointer_lib.dispatch(update.pointer)
  tooltip.update(update)
end)
"#;

/// A rig of `BODY` (and `extra`) whose first step marks the places and whose later steps are `steps`.
fn field_rig(extra: &str, steps: &[&str]) -> Rig {
    let mut all = vec!["marks()"];
    all.extend_from_slice(steps);
    let mut rig = rig(&format!("{BODY}\n{extra}"), &all);
    rig.opened();
    rig
}

/// Runs the script's next step (`report()`, say) and returns the `state` line it noted, with no `state ` prefix.
fn state(rig: &mut Rig) -> String {
    rig.opened();
    notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("state ").map(str::to_owned))
        .expect("the script noted a state")
}

/// The main thread applies the latest turning on of text input, as the loop does: what was typed before it is dropped.
pub(super) fn input_applied(rig: &mut Rig) {
    let generation = notes(rig)
        .iter()
        .rev()
        .find_map(|line| {
            line.strip_prefix("input true ")
                .map(|rest| rest.parse::<u64>().expect("a generation"))
        })
        .expect("text input was turned on");
    rig.mapper.reset_typed(generation);
    rig.feed(&[]);
}

pub(super) fn chars(text: &str) -> Vec<TypedEntry> {
    text.chars().map(TypedEntry::Char).collect()
}

pub(super) fn press_events(rig: &Rig, name: &str) -> Vec<PointerEvent> {
    let (x, y) = point(rig, name);
    vec![
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]
}

/// One update: its typing and its pointer events handed to the mapper (which keeps the two lists apart), one input tick, one frame.
pub(super) fn update(rig: &mut Rig, typed: &[TypedEntry], pointer: &[PointerEvent]) {
    for entry in typed {
        rig.mapper.typed(*entry);
    }
    rig.feed(pointer);
    frame(rig);
}

/// The same with the pointer handed to the mapper first.
pub(super) fn update_pointer_first(rig: &mut Rig, pointer: &[PointerEvent], typed: &[TypedEntry]) {
    for event in pointer {
        rig.mapper.pointer(*event);
    }
    for entry in typed {
        rig.mapper.typed(*entry);
    }
    rig.feed(&[]);
    frame(rig);
}

/// A press on a field starts its editing, and the main thread turns text input on.
pub(super) fn start(rig: &mut Rig, name: &str) {
    click(rig, name);
    input_applied(rig);
}

fn three_backspaces() -> [TypedEntry; 3] {
    [TypedEntry::Backspace; 3]
}

// ---- The look ----

#[test]
fn the_field_is_a_cut_3_frame_15_tall_filled_body_with_the_number_right_aligned_and_a_caret_only_while_it_edits()
 {
    let mut rig = field_rig(
        "",
        &[
            r#"
local f = fields.A
local width, height = f:size()
local zeros = font.layout("0000").width
eq(width, (zeros + 8) * P, "as wide as 0000 plus 8")
eq(height, 15 * P, "15 UI pixels tall")
eq(f.frame.fill, theme_lib.colour("body"), "filled body")
eq(f.frame.line, theme_lib.kind("window").outline, "outlined in the kind's outline")
eq(f.frame.cut, 3)
eq(shown("A"), "1920")
local ox, oy = content_origin()
local lx, ly = line_at(f.line)
eq(lx + f.line.width * P, ox + f.x + width - 4 * P, "the number ends 4 UI pixels short of the right edge")
eq(ly, oy + f.y + 2 * P, "and its text is 2 down")
eq(f.line.test_runs[1].colour, theme_lib.kind("window").text, "in text")
expect(f.caret == nil, "no caret before editing")
local x, y, w, h = hit_rect(f.key)
eq(w, width) eq(h, height)
"#,
            r#"
local f = fields.A
local width = f:size()
local ox, oy = content_origin()
eq(f.frame.line, theme_lib.kind("window").hot, "the editing outline is hot")
eq(f.frame.fill, theme_lib.colour("body"))
expect(f.caret ~= nil, "a caret stands")
local caret = scene.quads[f.caret]
local x, y, w, h = quad_at(caret)
eq(x, ox + f.x + width - 3 * P, "just right of the number")
eq(y, oy + f.y + 2 * P)
eq(w, P, "1 pixel wide")
eq(h, 11 * P, "11 tall")
eq(caret.tint, theme_lib.kind("window").text, "in text")
eq(shown("A"), "1920", "the number is kept")
"#,
            r#"
local f = fields.A
eq(f.frame.line, theme_lib.kind("window").outline, "the outline is back")
expect(f.caret == nil, "and the caret is gone")
local width = f:size()
local carets = 0
for _, quad in pairs(scene.quads) do
  if quad.group == W.content and quad.x == f.x + width - 3 * P and quad.y == f.y + 2 * P and quad.h == 11 * P and quad.w == P then
    carets = carets + 1
  end
end
eq(carets, 0, "no caret quad is left")
eq(shown("A"), "1920")
"#,
        ],
    );
    rig.opened();
    start(&mut rig, "A");
    rig.opened();
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened();
}

#[test]
fn a_field_may_ask_for_another_length_and_refuses_a_range_its_digits_cannot_show() {
    let mut rig = field_rig(
        "",
        &[r#"
local long = numberfield.new{x = 0, y = 40, value = 12345, least = 0, greatest = 99999, digits = 5, group = W.content, owner = W}
eq(long:size(), (font.layout("00000").width + 8) * P, "five digits are wider than four")
expect(long:size() > fields.A:size())
eq(line_text(long.line), "12345")
local ok, message = pcall(numberfield.new, {x = 0, y = 0, value = 1, least = 0, greatest = 12345, group = W.content, owner = W})
expect(not ok and message:find("more than 4 digits"), "a range wider than the digits is refused: " .. tostring(message))
expect(not pcall(numberfield.new, {x = 0, y = 0, value = -1, least = 0, greatest = 9, group = W.content, owner = W}), "a negative number")
expect(not pcall(numberfield.new, {x = 0, y = 0, value = 1.5, least = 0, greatest = 9, group = W.content, owner = W}), "a fraction")
expect(not pcall(numberfield.new, {x = 0, y = 0, value = 5, least = 9, greatest = 1, group = W.content, owner = W}), "least above greatest")
long:destroy()
"#],
    );
    rig.opened();
}

// ---- The typing ----

#[test]
fn a_click_starts_editing_with_the_number_kept_and_digits_and_backspaces_edit_its_end() {
    let mut rig = field_rig(
        "",
        &[
            "report()", "report()", "report()", "report()", "report()", "report()",
        ],
    );
    start(&mut rig, "A");
    assert_eq!(
        state(&mut rig),
        "applied= editing=A A=1920 B=480 clicks=0",
        "the number is kept"
    );
    // A digit into a full field does nothing; Backspace takes the last digit; digits typed quickly in one update arrive in order.
    update(&mut rig, &chars("5"), &[]);
    assert_eq!(state(&mut rig), "applied= editing=A A=1920 B=480 clicks=0");
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    assert_eq!(state(&mut rig), "applied= editing=A A=192 B=480 clicks=0");
    update(&mut rig, &chars("75"), &[]);
    assert_eq!(
        state(&mut rig),
        "applied= editing=A A=1927 B=480 clicks=0",
        "the first digit fills the field, the second does nothing"
    );
    // Three backspace entries in one update take three digits, whether typed or repeated by a held key.
    update(&mut rig, &three_backspaces(), &[]);
    assert_eq!(state(&mut rig), "applied= editing=A A=1 B=480 clicks=0");
    update(
        &mut rig,
        &[
            TypedEntry::Char('2'),
            TypedEntry::Char('0'),
            TypedEntry::Char('7'),
        ],
        &[],
    );
    assert_eq!(state(&mut rig), "applied= editing=A A=1207 B=480 clicks=0");
}

#[test]
fn a_backspace_on_an_empty_field_a_letter_a_left_and_a_right_change_nothing() {
    let mut rig = field_rig("", &["report()", "report()", "report()"]);
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace; 6], &[]);
    assert_eq!(
        state(&mut rig),
        "applied= editing=A A= B=480 clicks=0",
        "four digits go, then the rest do nothing"
    );
    update(
        &mut rig,
        &[
            TypedEntry::Char('x'),
            TypedEntry::Char('é'),
            TypedEntry::Char('-'),
            TypedEntry::Char('.'),
            TypedEntry::Char(' '),
            TypedEntry::Left,
            TypedEntry::Right,
        ],
        &[],
    );
    assert_eq!(state(&mut rig), "applied= editing=A A= B=480 clicks=0");
    // Digits go on after the junk, and a number pad's digit is the same character as the digit row's.
    update(&mut rig, &[TypedEntry::Left, TypedEntry::Char('9')], &[]);
    assert_eq!(state(&mut rig), "applied= editing=A A=9 B=480 clicks=0");
}

#[test]
fn a_field_emptied_and_then_applied_puts_its_old_number_back_and_tells_the_caller_nothing() {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace; 4], &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        "applied= editing=none A=1920 B=480 clicks=0"
    );
}

// ---- Applying ----

#[test]
fn enter_applies_the_number_brought_into_its_range_and_tells_the_caller_once() {
    let mut rig = field_rig("", &["report()", "report()", "report()", "report()"]);
    // Below the least: 640. Above the greatest: 7680. Inside: as typed. Unchanged: the caller is told nothing.
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace; 4], &[]);
    update(&mut rig, &chars("5"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640 editing=none A=640 B=480 clicks=0"
    );
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace; 3], &[]);
    update(&mut rig, &chars("9999"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640,A=7680 editing=none A=7680 B=480 clicks=0"
    );
    start(&mut rig, "A");
    update(
        &mut rig,
        &[TypedEntry::Backspace, TypedEntry::Char('0')],
        &[],
    );
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640,A=7680 editing=none A=7680 B=480 clicks=0",
        "the same number again: nobody is told"
    );
    start(&mut rig, "A");
    update(
        &mut rig,
        &[TypedEntry::Backspace, TypedEntry::Backspace],
        &[],
    );
    update(&mut rig, &chars("55"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640,A=7680,A=7655 editing=none A=7655 B=480 clicks=0"
    );
}

#[test]
fn tab_applies_and_does_nothing_more() {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    update(&mut rig, &[TypedEntry::Tab], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=1300 editing=none A=1300 B=480 clicks=0",
        "Tab applied the field and started no other"
    );
}

#[test]
fn a_press_on_another_widget_or_on_the_map_applies_the_field_and_the_press_still_acts() {
    let mut rig = field_rig("", &["report()", "report()"]);
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    let press = press_events(&rig, "button");
    update(&mut rig, &[], &press);
    assert_eq!(
        state(&mut rig),
        "applied=A=1300 editing=none A=1300 B=480 clicks=1",
        "the button's press applied the field, and the button still clicked"
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("200"), &[]);
    // The map: a press over no hit region.
    click(&mut rig, "map");
    assert_eq!(
        state(&mut rig),
        "applied=A=1300,A=1200 editing=none A=1200 B=480 clicks=1"
    );
}

#[test]
fn a_press_on_the_field_being_edited_applies_nothing_and_the_caller_is_told_once_however_it_ends() {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    click(&mut rig, "A");
    click(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Enter], &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    click(&mut rig, "map");
    assert_eq!(
        state(&mut rig),
        "applied=A=1300 editing=none A=1300 B=480 clicks=0"
    );
}

#[test]
fn a_press_of_the_middle_or_right_button_or_a_move_applies_nothing() {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    for name in ["map", "button", "B"] {
        let (x, y) = point(&rig, name);
        for button in [PointerButton::Middle, PointerButton::Right] {
            rig.feed(&[
                PointerEvent::ButtonDown {
                    button,
                    position: points(x, y),
                },
                PointerEvent::ButtonUp {
                    button,
                    position: points(x, y),
                },
            ]);
            frame(&mut rig);
        }
        rig.feed(&[PointerEvent::Move(points(x, y))]);
        frame(&mut rig);
    }
    assert_eq!(
        state(&mut rig),
        "applied= editing=A A=1 B=480 clicks=0",
        "only a left press applies"
    );
}

// ---- Two fields ----

#[test]
fn a_click_on_the_second_field_applies_the_first_and_starts_the_second_with_text_input_left_on() {
    let mut rig = field_rig("", &["report()", "report()"]);
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    click(&mut rig, "B");
    assert_eq!(
        state(&mut rig),
        "applied=A=1300 editing=B A=1300 B=480 clicks=0"
    );
    // Typing reaches only the field that edits.
    update(&mut rig, &chars("7"), &[]);
    update(&mut rig, &[TypedEntry::Char('1')], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=1300 editing=B A=1300 B=4807 clicks=0"
    );
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(
        calls,
        ["input true 1"],
        "text input stayed on through the change from one field to the other"
    );
    click(&mut rig, "map");
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(
        calls,
        ["input true 1", "input false 2"],
        "and went off when none edits"
    );
}

// ---- Typing and the pointer in one update (rule 8) ----

/// A rig with field A edited down to "1" (three backspaces in one update), B untouched, and one `report()` step.
fn scene() -> Rig {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    rig
}

#[test]
fn a_digit_then_a_click_on_field_b_in_one_update_gives_a_the_digit_and_applies_it_and_b_edits_untouched()
 {
    let mut rig = scene();
    let press = press_events(&rig, "B");
    update(&mut rig, &chars("207"), &press);
    assert_eq!(
        state(&mut rig),
        "applied=A=1207 editing=B A=1207 B=480 clicks=0"
    );
}

#[test]
fn a_click_on_field_b_then_a_digit_in_one_update_gives_the_same_by_the_rule() {
    let mut rig = scene();
    let press = press_events(&rig, "B");
    update_pointer_first(&mut rig, &press, &chars("207"));
    assert_eq!(
        state(&mut rig),
        "applied=A=1207 editing=B A=1207 B=480 clicks=0",
        "typing is handled first, all of it: the digits are A's"
    );
}

#[test]
fn digits_a_press_elsewhere_and_more_digits_in_one_update_give_a_every_digit_and_apply_it_once() {
    let mut rig = scene();
    let press = press_events(&rig, "button");
    rig.mapper.typed(TypedEntry::Char('2'));
    rig.mapper.typed(TypedEntry::Char('0'));
    for event in &press {
        rig.mapper.pointer(*event);
    }
    rig.mapper.typed(TypedEntry::Char('7'));
    rig.feed(&[]);
    frame(&mut rig);
    assert_eq!(
        state(&mut rig),
        "applied=A=1207 editing=none A=1207 B=480 clicks=1",
        "all three digits, one application, and the press still acted"
    );
}

#[test]
fn enter_then_a_digit_with_a_click_on_b_applies_a_without_the_digit_and_the_digit_reaches_no_field()
{
    let mut rig = scene();
    let press = press_events(&rig, "B");
    update(
        &mut rig,
        &[TypedEntry::Enter, TypedEntry::Char('7')],
        &press,
    );
    assert_eq!(
        state(&mut rig),
        "applied=A=640 editing=B A=640 B=480 clicks=0",
        "the Enter applied A with \"1\" (brought into its range); the digit was nobody's; B edits"
    );
}

#[test]
fn the_same_cases_in_two_updates_give_the_order_of_the_updates() {
    // A digit, then (next update) a click on B.
    let mut rig = scene();
    update(&mut rig, &chars("207"), &[]);
    let press = press_events(&rig, "B");
    update(&mut rig, &[], &press);
    assert_eq!(
        state(&mut rig),
        "applied=A=1207 editing=B A=1207 B=480 clicks=0"
    );
    // A click on B, then (next update) digits: the digits are B's, and A was applied without them.
    let mut rig = scene();
    let press = press_events(&rig, "B");
    update(&mut rig, &[], &press);
    update(&mut rig, &chars("207"), &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640 editing=B A=640 B=4802 clicks=0",
        "B had room for one digit, and the second does nothing"
    );
    // Digits, a press elsewhere, more digits, each in its own update: the last digit reaches no field.
    let mut rig = scene();
    update(&mut rig, &chars("20"), &[]);
    let press = press_events(&rig, "button");
    update(&mut rig, &[], &press);
    update(&mut rig, &chars("7"), &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640 editing=none A=640 B=480 clicks=1",
        "\"120\" is below the least; the 7 came after the press"
    );
    // Enter and a digit, then a click on B.
    let mut rig = scene();
    update(&mut rig, &[TypedEntry::Enter, TypedEntry::Char('7')], &[]);
    let press = press_events(&rig, "B");
    update(&mut rig, &[], &press);
    assert_eq!(
        state(&mut rig),
        "applied=A=640 editing=B A=640 B=480 clicks=0"
    );
    // A click on B, then Enter and a digit: the Enter applies B (unchanged: nobody is told) and the digit reaches no field.
    let mut rig = scene();
    let press = press_events(&rig, "B");
    update(&mut rig, &[], &press);
    update(&mut rig, &[TypedEntry::Enter, TypedEntry::Char('7')], &[]);
    assert_eq!(
        state(&mut rig),
        "applied=A=640 editing=none A=640 B=480 clicks=0"
    );
}

// ---- Escape ----

#[test]
fn escape_while_editing_puts_the_old_number_back_and_closes_nothing_and_the_next_escape_acts_as_before()
 {
    let mut rig = field_rig(
        PROBE,
        &[
            r#"
mark("probe", 630, 420)
"#,
            "report()",
            r#"
eq(count_hits("tooltip_panel_"), 1, "the tooltip is open over the probe")
"#,
            r#"
local f = fields.A
expect(not f:editing() and f.caret == nil, "the field does not edit")
eq(shown("A"), "1920", "the old number is back")
eq(f.frame.line, theme_lib.kind("window").outline)
eq(#applied, 0, "the caller was told nothing")
expect(not W.closed, "the window is open")
eq(count_hits("tooltip_panel_"), 1, "and so is the locked tooltip")
expect(typing.editing() == nil, "no typing editor")
"#,
            r#"
eq(count_hits("tooltip_panel_"), 0, "the second Escape closed the tooltip, as before")
expect(not W.closed)
"#,
            r#"
expect(W.closed, "the third closed the window")
"#,
        ],
    );
    rig.opened();
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("77"), &[]);
    assert_eq!(state(&mut rig), "applied= editing=A A=177 B=480 clicks=0");
    let probe = point(&rig, "probe");
    hover(&mut rig, probe);
    for _ in 0..150 {
        frame(&mut rig);
    }
    hover(&mut rig, (700.0, 550.0));
    rig.opened();
    assert!(rig.escape(), "Escape declines: the run goes on");
    rig.opened();
    assert!(
        notes(&rig).contains(&"input false 2".to_owned()),
        "text input is off again: {:?}",
        notes(&rig)
    );
    assert!(rig.escape(), "the next Escape closes the tooltip");
    rig.opened();
    assert!(rig.escape(), "and the one after, the window");
    rig.opened();
}

// ---- No field editing, the pause menu and the lifecycle ----

#[test]
fn with_no_field_editing_typed_digits_and_commands_change_nothing() {
    let mut rig = field_rig("", &["report()"]);
    update(
        &mut rig,
        &[
            TypedEntry::Char('1'),
            TypedEntry::Backspace,
            TypedEntry::Enter,
            TypedEntry::Tab,
            TypedEntry::Left,
        ],
        &[],
    );
    assert_eq!(
        state(&mut rig),
        "applied= editing=none A=1920 B=480 clicks=0"
    );
    assert!(
        !notes(&rig).iter().any(|line| line.starts_with("input ")),
        "text input was never turned on"
    );
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

#[test]
fn under_the_pause_menu_a_3_typed_into_an_editing_field_adds_a_3_and_its_speed_key_sends_nothing() {
    let mut rig = field_rig("", &["menu.open(VIEW)", "report()"]);
    // The game's Digit3 is bound to the speed action, as the shipped configuration binds it.
    rig.mapper = Mapper::new(
        Bindings::from_entries(vec![("time_speed_3".to_owned(), vec!["Digit3".to_owned()])])
            .expect("bindings"),
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    // With no menu the key is the game's: it sets the speed, and the field takes the character as well.
    rig.mapper.key(Key::Digit3, true, false);
    update(&mut rig, &chars("3"), &[]);
    assert_eq!(
        sent(&rig),
        [Control::Speed(3)],
        "the key acts with no block"
    );
    rig.mapper.key(Key::Digit3, false, false);
    rig.feed(&[]);
    // The menu opens (and pauses): the same key and character again.
    rig.opened();
    assert!(rig.controls.keys_blocked(), "the menu turned the block on");
    let _ = sent(&rig);
    rig.mapper.key(Key::Digit3, true, false);
    update(&mut rig, &chars("3"), &[]);
    assert!(
        sent(&rig).is_empty(),
        "the speed action is stopped at the simulation"
    );
    assert_eq!(
        state(&mut rig),
        "applied= editing=A A=133 B=480 clicks=0",
        "and the field took the 3"
    );
}

#[test]
fn closing_the_window_while_a_field_edits_applies_nothing_and_leaves_no_caret_key_or_editing_state()
{
    let mut rig = field_rig(
        "",
        &[
            "W:close()",
            r#"
eq(#applied, 0, "nothing was applied")
expect(numberfield.current() == nil, "no field edits")
expect(typing.editing() == nil, "the typing function has no editor")
expect(fields.A.caret == nil and fields.A.frame == nil, "no caret, and the field is gone")
eq(count_hits("numberfield_"), 0, "no hit key is left")
fields.A:destroy()
"#,
        ],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    rig.opened();
    assert!(
        notes(&rig).contains(&"input false 2".to_owned()),
        "text input went off: {:?}",
        notes(&rig)
    );
    rig.opened();
    // What is typed now reaches no field, and a press where the field was is not a press on it.
    update(&mut rig, &chars("5"), &[]);
    click_at(&mut rig, (60.0, 60.0));
}

#[test]
fn escape_with_no_field_editing_acts_as_before() {
    let mut rig = field_rig("", &["expect(W.closed, \"Escape closed the window\")"]);
    assert!(rig.escape());
    rig.opened();
}

// ---- A caller's callback that destroys fields while a handoff is under way ----

/// The state a callback-destroy leaves, as the script sees it, for the two tests below.
const GONE: &str = r#"
expect(numberfield.current() == nil, "no field edits")
expect(typing.editing() == nil, "the typing function has no editor")
for name, f in pairs(fields) do
  if f.frame == nil then expect(f.caret == nil, name .. " has no caret") end
end
expect(fields.B.frame == nil and fields.B.caret == nil, "B is gone and drew nothing")
eq(count_hits("numberfield_"), 0 + (fields.A.frame ~= nil and 1 or 0), "no hit key is left for a destroyed field")
"#;

#[test]
fn an_apply_callback_that_closes_the_window_during_a_handoff_leaves_no_editor_and_text_input_off() {
    let mut rig = field_rig(
        "function after_apply(name) if name == \"A\" then W:close() end end",
        &[&format!(
            r#"
{GONE}
eq(table.concat(applied, ","), "A=1300", "A's callback ran once")
expect(W.closed, "the window closed")
"#
        )],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    // The press on B applies A, whose callback closes the window that holds B: no handoff, and no error in the frame.
    click(&mut rig, "B");
    rig.opened();
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(
        calls,
        ["input true 1", "input false 2"],
        "text input went off and was not turned on again"
    );
    // What is typed now reaches no field.
    update(&mut rig, &chars("5"), &[]);
}

#[test]
fn an_apply_callback_that_destroys_the_target_field_during_a_handoff_leaves_no_editor_and_text_input_off()
 {
    let mut rig = field_rig(
        "function after_apply(name) if name == \"A\" then fields.B:destroy() end end",
        &[&format!(
            r#"
{GONE}
eq(table.concat(applied, ","), "A=1300", "A's callback ran once")
expect(not W.closed, "the window is open")
eq(shown("A"), "1300", "A shows what was applied")
"#
        )],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    click(&mut rig, "B");
    rig.opened();
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(calls, ["input true 1", "input false 2"]);
    // A is live and can be edited again.
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
}

#[test]
fn an_apply_callback_that_restarts_the_previous_field_leaves_it_editing_with_typing_reaching_it() {
    let mut rig = field_rig(
        r#"
restarted = false
function after_apply(name)
  if name == "A" and not restarted then
    restarted = true
    fields.A:on_event({kind = "down"}, true)
  end
end
"#,
        &[
            r#"
eq(table.concat(applied, ","), "A=1300", "A's callback ran once, for what was typed")
expect(numberfield.current() == fields.A and fields.A:editing() and fields.A.caret ~= nil, "A edits again, with its caret")
expect(typing.editing() == fields.A, "and the typing function names A: the field and the library agree")
expect(not fields.B:editing() and fields.B.caret == nil, "B does not edit")
"#,
            r#"
eq(table.concat(applied, ","), "A=1300,A=1305", "what is typed afterwards reached A")
expect(numberfield.current() == nil and typing.editing() == nil, "after the Enter nobody edits")
expect(fields.A.caret == nil and not fields.A:editing(), "and A shows no editing")
"#,
        ],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    // The press on B settles A and starts B; A's callback restarts A.
    click(&mut rig, "B");
    rig.opened();
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(calls, ["input true 1"], "text input stayed on throughout");
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    update(&mut rig, &chars("5"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened();
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(calls, ["input true 1", "input false 2"]);
}

#[test]
fn an_apply_callback_that_starts_a_third_field_leaves_that_one_editing_with_typing_reaching_it() {
    let mut rig = field_rig(
        r#"
add("C", 200, 50, 0, 999)
function after_apply(name)
  if name == "A" then fields.C:on_event({kind = "down"}, true) end
end
"#,
        &[
            r#"
eq(table.concat(applied, ","), "A=1300")
expect(numberfield.current() == fields.C and fields.C:editing() and fields.C.caret ~= nil, "C edits")
expect(typing.editing() == fields.C, "and the typing function names C")
expect(not fields.A:editing() and not fields.B:editing(), "neither A nor B does")
expect(fields.A.caret == nil and fields.B.caret == nil, "and neither has a caret")
"#,
            r#"
eq(table.concat(applied, ","), "A=1300,C=507", "what is typed afterwards reached C")
expect(numberfield.current() == nil and typing.editing() == nil)
"#,
        ],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    click(&mut rig, "B");
    rig.opened();
    update(&mut rig, &chars("7"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened();
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(calls, ["input true 1", "input false 2"]);
}

// ---- Key capture (CK-309): the typing library's capture protocol with a real field ----

/// The Lua the capture tests share: a capture request that records what the state was when it was called.
const CAPTURE: &str = r#"
request_log = {}
function arm(id)
  typing.capture(id, function(asked)
    request_log[#request_log + 1] = string.format("%s applied=%s current=%s editor=%s", asked, table.concat(applied, ","),
      tostring(numberfield.current() ~= nil), tostring(typing.editing() ~= nil))
  end)
end
"#;

#[test]
fn arming_a_capture_while_a_field_edits_applies_it_once_with_the_number_and_asks_for_the_capture_after()
 {
    let mut rig = field_rig(
        CAPTURE,
        &[r#"
arm("cap")
eq(#request_log, 1, "the capture request was made once")
eq(request_log[1], "cap applied=A=1300 current=false editor=false",
  "the field was applied (with the number 1300) and ended before the request was made")
eq(table.concat(applied, ","), "A=1300", "the caller was told once")
expect(not fields.A:editing() and fields.A.caret == nil, "no editing state or caret is left")
eq(fields.A.frame.line, theme_lib.kind("window").outline, "and the outline is back")
eq(shown("A"), "1300")
expect(numberfield.current() == nil and typing.editing() == nil, "nobody edits")
"#],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    rig.opened();
    assert!(
        notes(&rig).contains(&"input false 2".to_owned()),
        "text input went off: {:?}",
        notes(&rig)
    );
}

#[test]
fn arming_a_capture_applies_an_unchanged_or_emptied_field_silently_and_a_field_with_no_callback_without_error()
 {
    let mut rig = field_rig(
        CAPTURE,
        &[
            r#"
arm("one")
eq(#applied, 0, "an unchanged field tells nobody")
eq(shown("A"), "1920")
expect(numberfield.current() == nil and typing.editing() == nil)
"#,
            r#"
-- The first capture completes (its result arrives with an update), so a field may start again.
typing.dispatch({capture = {id = "one"}})
-- A field made with no on_apply, edited directly through its widget entry points.
local bare = numberfield.new{x = 200, y = 0, value = 5, least = 0, greatest = 99, group = W.content, owner = W}
bare:on_event({kind = "down"}, true)
bare:on_typed({kind = "char", text = "7"})
expect(typing.editing() == bare and bare:editing(), "the bare field edits")
arm("two")
eq(bare:get(), 57, "it was applied")
expect(not bare:editing() and bare.caret == nil and numberfield.current() == nil and typing.editing() == nil, "and ended")
eq(#request_log, 2, "both requests were made")
"#,
        ],
    );
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    update(&mut rig, &chars("0"), &[]);
    rig.opened();
    rig.opened();
}

#[test]
fn a_caller_that_restarts_editing_while_the_capture_is_armed_leaves_no_field_editing() {
    let mut rig = field_rig(
        &format!(
            r#"{CAPTURE}
restarted = false
function after_apply(name)
  if name == "A" and not restarted then
    restarted = true
    fields.A:on_event({{kind = "down"}}, true)
  end
end
"#
        ),
        &[r#"
arm("cap")
eq(#request_log, 1)
eq(request_log[1], "cap applied=A=1300 current=false editor=false", "nothing edited when the capture was requested")
expect(restarted, "the caller did try to restart the field")
expect(not fields.A:editing() and fields.A.caret == nil, "the restarted editing was ended: the keyboard is the capture's")
expect(numberfield.current() == nil and typing.editing() == nil, "no field and no typing owner survive")
eq(shown("A"), "1300")
"#],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    rig.opened();
    assert!(
        notes(&rig).contains(&"input false 2".to_owned()),
        "text input is off: {:?}",
        notes(&rig)
    );
}

#[test]
fn a_press_on_a_field_while_a_capture_is_armed_starts_nothing_and_a_fresh_edit_works_after_it_completes()
 {
    let mut rig = field_rig(
        CAPTURE,
        &[
            r#"arm("cap")"#,
            r#"
local f = fields.A
expect(not f:editing() and f.caret == nil, "the refused press left no editing state or caret")
expect(numberfield.current() == nil and typing.editing() == nil, "no current field and no typing owner")
eq(f.frame.line, theme_lib.kind("window").outline, "the outline is not hot")
eq(#applied, 0, "nothing was applied as a side effect")
-- The capture's result arrives with an update: the library lets go of the keyboard.
typing.dispatch({capture = {id = "cap"}})
"#,
            "report()",
        ],
    );
    rig.opened();
    click(&mut rig, "A");
    rig.opened();
    assert!(
        !notes(&rig).iter().any(|line| line.starts_with("input ")),
        "text input was never turned on by the refused press: {:?}",
        notes(&rig)
    );
    // After the capture completed a press starts the editing, and typing reaches it.
    start(&mut rig, "A");
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    assert_eq!(state(&mut rig), "applied= editing=A A=192 B=480 clicks=0");
}

// The two probes of review 4 (`ck306-review-4-capture-probes.rs`), kept as they were written.

#[test]
fn review_probe_arming_capture_applies_the_real_numberfield_and_clears_its_editing() {
    let mut rig = field_rig(
        "",
        &[r#"
capture_requested = false
typing.capture("review_capture", function(id)
  eq(id, "review_capture")
  expect(numberfield.current() == nil and typing.editing() == nil, "editing ended before capture request")
  expect(not fields.A:editing() and fields.A.caret == nil, "the field stopped showing editing")
  eq(shown("A"), "1300", "the field shows its applied number")
  eq(table.concat(applied, ","), "A=1300", "the caller got one numeric application")
  capture_requested = true
end)
expect(capture_requested, "the capture was requested after settling")
"#],
    );
    start(&mut rig, "A");
    update(&mut rig, &three_backspaces(), &[]);
    update(&mut rig, &chars("300"), &[]);
    rig.opened();
}

#[test]
fn review_probe_a_pending_capture_refuses_numberfield_editing_including_its_visible_state() {
    let mut rig = field_rig(
        "typing.capture('review_capture', function(id) eq(id, 'review_capture') end)",
        &[r#"
expect(typing.editing() == nil, "capture owns the keyboard")
expect(numberfield.current() == nil and not fields.A:editing(), "capture refused starting a field")
expect(fields.A.caret == nil, "no editing caret during capture")
eq(fields.A.frame.line, fields.A.colours.outline, "no hot editing outline during capture")
eq(#applied, 0)
"#],
    );
    click(&mut rig, "A");
    rig.opened();
    assert!(!notes(&rig).iter().any(|x| x.starts_with("input true ")));
}
