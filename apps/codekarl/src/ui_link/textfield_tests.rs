//! The text field (CK-303, `scripts/ui/lib/textfield.lua`) in a real UI VM, in the script test rig, with no window and no GPU.
//!
//! The shipped library runs over the real `render` API and the shipped font, theme and art; the scene shadow of `listbar_tests`'s prelude
//! records every quad and text line. Typing is real: the loop's mapper gathers typed entries, the tick carries them to the link, and the
//! script's frame hook runs `typing.dispatch` before `pointer.dispatch`, as `main.lua` does. Pointer cases press real regions. A test reads
//! the fields' state from a line the script notes (`state ...`). Nothing sleeps.

use ck_input::{Bindings, Key, Mapper};
use ck_window::TypedEntry;

use super::group_tests::Rig;
use super::listbar_tests::{PROBE, click, click_at, frame, hover, notes, point, rig};
use super::numberfield_tests::{chars, press_events, sent, start, update, update_pointer_first};
use crate::game::controls::Control;

/// Three text fields (`T`, empty, 12 characters, 60 UI pixels wide; `U`, `wxyz`, the same; `W`, empty, 48 characters, 190 wide), a number
/// field `N` (1920 in 640 to 7680), a button that counts its clicks, the spy on `window.set_text_input`, and the frame hook of `main.lua`
/// (typing, pointer, tooltips).
const BODY: &str = r#"
numberfield = require("lib.numberfield")
textfield = require("lib.textfield")
typing = require("lib.typing")
menu = require("lib.menu")
VIEW = {width = 800, height = 600}
local real_set_text_input = window.set_text_input
window.set_text_input = function(on)
  local generation = real_set_text_input(on)
  note("input " .. tostring(on) .. " " .. generation)
  return generation
end
new_window(200, 100, "window")
fields, applied, clicks = {}, {}, 0
-- What a caller's `on_apply` does besides noting the text; a test replaces it (to close the window, say).
function after_apply() end
function add_text(name, x, y, width, value, greatest)
  fields[name] = textfield.new{x = x, y = y, width = width * P, value = value, greatest = greatest,
    group = W.content, owner = W, on_apply = function(t) applied[#applied + 1] = name .. "=" .. t after_apply(name) end}
end
add_text("T", 0, 0, 60, "", 12)
add_text("U", 0, 40, 60, "wxyz", 12)
add_text("W", 0, 80, 190, "", 48)
fields.N = numberfield.new{x = 140, y = 0, value = 1920, least = 640, greatest = 7680, group = W.content, owner = W,
  on_apply = function(v) applied[#applied + 1] = "N=" .. v after_apply("N") end}
shove = button.new{x = 0, y = 130, text = "elsewhere", group = W.content, owner = W, on_click = function() clicks = clicks + 1 end}
-- What a text field holds as typed (while editing) or as applied.
function buf(name) return table.concat(fields[name].chars) end
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
  note(string.format("state applied=%s editing=%s T=<%s> U=<%s> N=%s clicks=%d", table.concat(applied, ","), who, buf("T"), buf("U"),
    line_text(fields.N.line), clicks))
end
-- Where the caret of a field that edits is wanted: 3 UI pixels in, then the width of the text before it.
function caret_wanted(f)
  local ox, oy = content_origin()
  return ox + f.x + (3 + font.layout(table.concat(f.chars, "", f.first, f.position)).width) * P, oy + f.y + 2 * P
end
function check_caret(f, label)
  expect(f.caret ~= nil, (label or "") .. " a caret stands")
  local x, y, w, h = quad_at(scene.quads[f.caret])
  local wx, wy = caret_wanted(f)
  eq(x, wx, (label or "") .. " caret x")
  eq(y, wy, (label or "") .. " caret y")
  eq(w, P) eq(h, 11 * P)
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

/// The state line for the given parts.
fn wanted(applied: &str, editing: &str, t: &str, u: &str, n: &str, clicks: u32) -> String {
    format!("applied={applied} editing={editing} T=<{t}> U=<{u}> N={n} clicks={clicks}")
}

fn typed(text: &str) -> Vec<TypedEntry> {
    chars(text)
}

// ---- The look ----

#[test]
fn the_field_is_the_number_fields_frame_as_wide_as_asked_with_its_text_left_aligned_and_a_caret_at_its_end_only_while_it_edits()
 {
    let mut rig = field_rig(
        "",
        &[
            r#"
local f = fields.U
local width, height = f:size()
eq(width, 60 * P, "as wide as it was asked")
eq(height, 15 * P, "15 UI pixels tall")
eq(f.frame.fill, theme_lib.colour("body"), "filled body")
eq(f.frame.line, theme_lib.kind("window").outline, "outlined in the kind's outline")
eq(f.frame.cut, 3)
eq(shown("U"), "wxyz")
local ox, oy = content_origin()
local lx, ly = line_at(f.line)
eq(lx, ox + f.x + 3 * P, "the text starts 3 UI pixels from the left edge")
eq(ly, oy + f.y + 2 * P, "and is 2 down")
eq(f.line.test_runs[1].colour, theme_lib.kind("window").text, "in text")
expect(f.caret == nil, "no caret before editing")
local x, y, w, h = hit_rect(f.key)
eq(w, width) eq(h, height)
"#,
            r#"
local f = fields.U
eq(f.frame.line, theme_lib.kind("window").hot, "the editing outline is hot")
eq(f.frame.fill, theme_lib.colour("body"))
check_caret(f, "at the end:")
eq(f.position, 4, "the caret starts at the end of the text")
eq(scene.quads[f.caret].tint, theme_lib.kind("window").text, "in text")
local x = quad_at(scene.quads[f.caret])
local ox = content_origin()
eq(x, ox + f.x + (3 + font.layout("wxyz").width) * P, "just after the text")
eq(shown("U"), "wxyz", "the text is kept")
"#,
            r#"
local f = fields.U
eq(f.frame.line, theme_lib.kind("window").outline, "the outline is back")
expect(f.caret == nil, "and the caret is gone")
eq(shown("U"), "wxyz")
"#,
        ],
    );
    rig.opened();
    start(&mut rig, "U");
    rig.opened();
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened();
}

#[test]
fn a_field_refuses_a_width_a_length_or_a_text_it_cannot_hold() {
    let mut rig = field_rig(
        "",
        &[r#"
local function refuses(spec, message)
  spec.group, spec.owner, spec.x, spec.y = W.content, W, 0, 0
  local ok, err = pcall(textfield.new, spec)
  expect(not ok and tostring(err):find(message, 1, true), message .. ": " .. tostring(err))
end
refuses({width = 10, greatest = 4}, "width")
refuses({width = 60 * P, greatest = 0}, "greatest")
refuses({width = 60 * P, greatest = 2.5}, "greatest")
refuses({width = 60 * P, greatest = 2, value = "abc"}, "longer than 2 characters")
refuses({width = 60 * P, greatest = 5, value = "a\xffb"}, "valid UTF-8")
refuses({width = 60 * P, greatest = 5, value = "aŵ"}, "cannot draw")
refuses({width = 60 * P, greatest = 5, value = 5}, "valid UTF-8")
local ok, err = pcall(fields.T.set, fields.T, "abcdefghijklm")
expect(not ok and tostring(err):find("longer than 12", 1, true), tostring(err))
ok, err = pcall(fields.T.set, fields.T, "a\u{1F600}")
expect(not ok and tostring(err):find("cannot draw", 1, true), tostring(err))
local longer = textfield.new{x = 0, y = 0, width = 60 * P, greatest = 3, value = "áéí", group = W.content, owner = W}
eq(longer:get(), "áéí", "three accented vowels fill a field of 3")
longer:destroy()
"#],
    );
    rig.opened();
}

// ---- Typed text ----

#[test]
fn letters_capitals_spaces_punctuation_and_an_accented_vowel_are_typed_and_a_character_without_a_glyph_is_refused()
 {
    let mut rig = field_rig("", &["report()", "report()", "report()", "report()"]);
    start(&mut rig, "T");
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "", "wxyz", "1920", 0),
        "the field starts empty"
    );
    update(&mut rig, &typed("Ab c,.é"), &[]);
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "Ab c,.é", "wxyz", "1920", 0)
    );
    // A character the font has no glyph for, an emoji, a euro sign and the control characters change nothing.
    update(
        &mut rig,
        &[
            TypedEntry::Char('ŵ'),
            TypedEntry::Char('€'),
            TypedEntry::Char('\u{1F600}'),
            TypedEntry::Char('\n'),
            TypedEntry::Char('\t'),
            TypedEntry::Char('\u{7f}'),
            TypedEntry::Char('\u{0}'),
            TypedEntry::Char('\u{85}'),
        ],
        &[],
    );
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "Ab c,.é", "wxyz", "1920", 0),
        "nothing was inserted"
    );
    // The refused characters did not stop the ones after them.
    update(
        &mut rig,
        &[TypedEntry::Char('ŵ'), TypedEntry::Char('Z')],
        &[],
    );
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "Ab c,.éZ", "wxyz", "1920", 0)
    );
}

#[test]
fn the_printable_ascii_set_and_the_irish_vowels_are_characters_the_font_draws_and_the_field_holds()
{
    let mut rig = field_rig(
        "",
        &[r#"
local f = fields.W
local all = {}
for code = 0x20, 0x7E do
  local character = utf8.char(code)
  local glyph = font.layout(character).glyphs[1]
  expect(code == 0x20 or (glyph ~= nil and not glyph.texture:find("replacement", 1, true)), "the font draws " .. character)
  all[#all + 1] = character
end
-- A field of 48 holds them in two goes.
f:set(table.concat(all, "", 1, 47))
eq(f:get(), table.concat(all, "", 1, 47))
f:set(table.concat(all, "", 48))
eq(f:get(), table.concat(all, "", 48))
local vowels = "ÁÉÍÓÚáéíóú·"
f:set(vowels)
eq(f:get(), vowels)
eq(utf8.len(f:get()), 11)
"#],
    );
    rig.opened();
}

#[test]
fn with_no_field_editing_typed_characters_and_commands_change_nothing() {
    let mut rig = field_rig("", &["report()"]);
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Left,
            TypedEntry::Right,
            TypedEntry::Backspace,
            TypedEntry::Enter,
            TypedEntry::Tab,
        ],
        &[],
    );
    assert_eq!(state(&mut rig), wanted("", "none", "", "wxyz", "1920", 0));
}

// ---- Characters, not bytes ----

#[test]
fn forty_eight_accented_vowels_fill_a_field_of_48_and_a_49th_character_is_refused() {
    let mut rig = field_rig(
        "",
        &[
            r#"
local s = buf("W")
eq(utf8.len(s), 48, "48 characters")
eq(#s, 96, "which are 96 bytes")
eq(s, string.rep("á", 48))
"#,
            r#"
local s = buf("W")
eq(utf8.len(s), 48, "a 49th character was refused, an accented vowel or not")
eq(s, string.rep("á", 48))
"#,
            r#"
local s = buf("W")
eq(utf8.len(s), 47)
expect(utf8.len(s) ~= nil and #s == 94, "Backspace took a vowel whole: 94 bytes of valid UTF-8")
"#,
            r#"
local s = buf("W")
eq(utf8.len(s), 48, "and the room is one character")
eq(s, string.rep("á", 47) .. "x")
"#,
        ],
    );
    start(&mut rig, "W");
    update(&mut rig, &typed(&"á".repeat(48)), &[]);
    rig.opened();
    update(&mut rig, &typed("éx"), &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    rig.opened();
    update(&mut rig, &typed("xy"), &[]);
    rig.opened();
}

#[test]
fn left_and_right_step_over_an_accented_vowel_in_one_move_and_the_caret_is_by_the_width_of_the_text_before_it()
 {
    let mut rig = field_rig(
        "",
        &[
            r#"
local f = fields.T
eq(buf("T"), "aéb")
eq(f.position, 3, "three characters before the caret")
check_caret(f, "at the end:")
"#,
            r#"
local f = fields.T
eq(f.position, 2, "Left moved over `b`")
check_caret(f, "after é:")
"#,
            r#"
local f = fields.T
eq(f.position, 1, "Left moved over the whole of `é`: one move, one character")
check_caret(f, "after a:")
local x = quad_at(scene.quads[f.caret])
local ox = content_origin()
eq(x, ox + f.x + (3 + font.layout("a").width) * P, "by the width of `a`")
"#,
            r#"
local f = fields.T
eq(f.position, 2, "Right moved over the whole of `é`")
check_caret(f, "after é again:")
eq(buf("T"), "aéb", "the text is not changed by moving")
eq(utf8.len(buf("T")), 3)
"#,
        ],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("aéb"), &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Right], &[]);
    rig.opened();
}

#[test]
fn backspace_after_an_accented_vowel_takes_it_whole_and_the_text_is_valid_utf8_after_each_edit() {
    let mut rig = field_rig(
        "",
        &[
            r#"
eq(buf("T"), "aé")
expect(utf8.len(buf("T")) == 2 and #buf("T") == 3, "two characters, three bytes")
"#,
            r#"
eq(buf("T"), "a", "the vowel went whole")
expect(utf8.len(buf("T")) == 1 and #buf("T") == 1, "valid UTF-8")
check_caret(fields.T, "after the vowel went:")
"#,
            r#"
eq(buf("T"), "éa")
"#,
            r#"
eq(buf("T"), "a", "Backspace with the caret after a vowel in the middle took the vowel")
expect(utf8.len(buf("T")) == 1, "valid UTF-8")
eq(fields.T.position, 0)
"#,
            r#"
eq(buf("T"), "a", "Backspace at the start does nothing")
"#,
        ],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("aé"), &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    update(&mut rig, &typed("éa"), &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left, TypedEntry::Backspace], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    rig.opened();
}

#[test]
fn a_character_typed_with_the_caret_in_the_middle_goes_in_there() {
    let mut rig = field_rig("", &["report()", "report()"]);
    start(&mut rig, "T");
    update(&mut rig, &typed("ab"), &[]);
    update(&mut rig, &[TypedEntry::Left, TypedEntry::Char('x')], &[]);
    assert_eq!(state(&mut rig), wanted("", "T", "axb", "wxyz", "1920", 0));
    update(
        &mut rig,
        &[
            TypedEntry::Left,
            TypedEntry::Left,
            TypedEntry::Char('é'),
            TypedEntry::Right,
            TypedEntry::Right,
            TypedEntry::Char('z'),
        ],
        &[],
    );
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "éaxzb", "wxyz", "1920", 0),
        "an accented vowel in the middle, then a character after the next two"
    );
}

// ---- The caret ----

#[test]
fn the_caret_follows_typing_left_right_and_backspace_and_nothing_happens_at_the_two_ends() {
    let mut rig = field_rig(
        "",
        &[
            "check_caret(fields.T, 'empty:') eq(fields.T.position, 0)",
            "check_caret(fields.T, 'after ab:') eq(fields.T.position, 2)",
            "check_caret(fields.T, 'after one Left:') eq(fields.T.position, 1)",
            "check_caret(fields.T, 'at the start:') eq(fields.T.position, 0)",
            "check_caret(fields.T, 'Left at the start does nothing:') eq(fields.T.position, 0) eq(buf('T'), 'ab')",
            "check_caret(fields.T, 'Backspace at the start does nothing:') eq(fields.T.position, 0) eq(buf('T'), 'ab')",
            "check_caret(fields.T, 'at the end again:') eq(fields.T.position, 2)",
            "check_caret(fields.T, 'Right at the end does nothing:') eq(fields.T.position, 2) eq(buf('T'), 'ab')",
            "check_caret(fields.T, 'after Backspace:') eq(fields.T.position, 1) eq(buf('T'), 'a')",
        ],
    );
    start(&mut rig, "T");
    rig.opened();
    update(&mut rig, &typed("ab"), &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Right; 3], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Right], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    rig.opened();
}

#[test]
fn the_caret_of_an_unmoved_field_follows_it() {
    let mut rig = field_rig(
        "",
        &[r#"
local f = fields.U
local ox, oy = content_origin()
f:move(20, 60)
check_caret(f, "after move:")
eq(select(1, line_at(f.line)), ox + 20 + 3 * P)
local x, y, w, h = hit_rect(f.key)
eq(x, ox + 20) eq(y, oy + 60)
"#],
    );
    start(&mut rig, "U");
    rig.opened();
}

// ---- Scrolling ----

#[test]
fn text_wider_than_the_field_scrolls_so_the_caret_stays_in_view_and_left_back_to_the_start_shows_the_start()
 {
    let mut rig = field_rig(
        "",
        &[
            r#"
local f = fields.T
local whole = "abcdefghijkl"
eq(buf("T"), whole)
expect(font.layout(whole).width > 60 - 7, "the text is wider than the field")
expect(f.first > 1, "the start has scrolled out")
local shown_text = shown("T")
expect(#shown_text < #whole, "not all of it is drawn")
eq(shown_text:sub(-1), "l", "the end is in view, with the caret after it")
eq(whole:sub(f.first, f.first + #shown_text - 1), shown_text, "and what is drawn is a run of the text")
check_caret(f, "at the end:")
local ox = content_origin()
local x = quad_at(scene.quads[f.caret])
expect(x >= ox + f.x + 3 * P and x + P <= ox + f.x + 60 * P - 3 * P, "the caret is inside the field's room")
local lx = line_at(f.line)
expect(lx + f.line.width * P <= ox + f.x + 60 * P - 3 * P, "so is the text")
"#,
            r#"
local f = fields.T
eq(f.position, 11)
check_caret(f, "after one Left:")
eq(shown("T"):sub(-1), "l", "the text did not move for a caret that is still in view")
"#,
            r#"
local f = fields.T
eq(f.position, 0)
eq(f.first, 1, "Left back to the start brings the start into view")
eq(shown("T"):sub(1, 1), "a")
check_caret(f, "at the start:")
local ox = content_origin()
eq(quad_at(scene.quads[f.caret]), ox + f.x + 3 * P)
"#,
            r#"
local f = fields.T
eq(f.position, 12)
expect(f.first > 1, "Right to the end scrolls again")
eq(shown("T"):sub(-1), "l")
check_caret(f, "at the end again:")
"#,
            r#"
local f = fields.T
eq(shown("T"), ("abcdefghijkl"):sub(1, #shown("T")), "applied, the field shows the start of its text")
eq(f.first, 1)
expect(f.caret == nil)
eq(fields.T:get(), "abcdefghijkl")
"#,
        ],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("abcdefghijkl"), &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Left; 11], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Right; 12], &[]);
    rig.opened();
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened();
}

#[test]
fn deleting_from_a_scrolled_field_brings_the_text_back_to_fill_it() {
    let mut rig = field_rig(
        "",
        &[r#"
local f = fields.T
eq(buf("T"), "abc")
eq(f.first, 1, "a text that fits is shown from its start")
eq(shown("T"), "abc")
check_caret(f, "at the end:")
"#],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("abcdefghijkl"), &[]);
    update(&mut rig, &[TypedEntry::Backspace; 9], &[]);
    rig.opened();
}

// ---- Order ----

#[test]
fn a_character_backspace_and_a_character_in_one_update_leave_the_second() {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "T");
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Backspace,
            TypedEntry::Char('b'),
        ],
        &[],
    );
    assert_eq!(state(&mut rig), wanted("", "T", "b", "wxyz", "1920", 0));
}

#[test]
fn two_characters_left_and_a_character_in_one_update_leave_a_x_b() {
    let mut rig = field_rig("", &["report()"]);
    start(&mut rig, "T");
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Char('b'),
            TypedEntry::Left,
            TypedEntry::Char('x'),
        ],
        &[],
    );
    assert_eq!(state(&mut rig), wanted("", "T", "axb", "wxyz", "1920", 0));
}

#[test]
fn a_character_enter_and_a_character_in_one_update_apply_with_the_first_only_and_the_second_reaches_no_field()
 {
    let mut rig = field_rig("", &["report()", "report()"]);
    start(&mut rig, "T");
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Enter,
            TypedEntry::Char('b'),
        ],
        &[],
    );
    assert_eq!(
        state(&mut rig),
        wanted("T=a", "none", "a", "wxyz", "1920", 0),
        "applied with `a` once, and `b` was nobody's"
    );
    update(&mut rig, &typed("c"), &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=a", "none", "a", "wxyz", "1920", 0)
    );
}

#[test]
fn the_same_orders_spread_over_two_updates_give_the_same_text() {
    let mut rig = field_rig("", &["report()", "report()", "report()"]);
    start(&mut rig, "T");
    update(&mut rig, &[TypedEntry::Char('a')], &[]);
    update(
        &mut rig,
        &[TypedEntry::Backspace, TypedEntry::Char('b')],
        &[],
    );
    assert_eq!(state(&mut rig), wanted("", "T", "b", "wxyz", "1920", 0));
    update(&mut rig, &[TypedEntry::Char('c'), TypedEntry::Left], &[]);
    update(&mut rig, &[TypedEntry::Char('x')], &[]);
    assert_eq!(state(&mut rig), wanted("", "T", "bxc", "wxyz", "1920", 0));
    update(&mut rig, &[TypedEntry::Char('d'), TypedEntry::Enter], &[]);
    update(&mut rig, &[TypedEntry::Char('e')], &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=bxdc", "none", "bxdc", "wxyz", "1920", 0),
        "the character of the next update reaches no field"
    );
}

// ---- With the pointer ----

/// Field `T` holds `ab` and edits.
fn scene() -> Rig {
    let mut rig = field_rig("", &["report()", "report()", "report()"]);
    start(&mut rig, "T");
    update(&mut rig, &typed("ab"), &[]);
    rig
}

#[test]
fn characters_and_a_press_elsewhere_in_one_update_apply_the_field_with_the_characters_in_either_order()
 {
    let mut rig = scene();
    let press = press_events(&rig, "button");
    update(&mut rig, &typed("cd"), &press);
    assert_eq!(
        state(&mut rig),
        wanted("T=abcd", "none", "abcd", "wxyz", "1920", 1),
        "the characters and then the press: the press acted"
    );
    let mut rig = scene();
    let press = press_events(&rig, "button");
    update_pointer_first(&mut rig, &press, &typed("cd"));
    assert_eq!(
        state(&mut rig),
        wanted("T=abcd", "none", "abcd", "wxyz", "1920", 1),
        "the press first in the mapper, the same by the rule: typing is handled first"
    );
}

#[test]
fn characters_around_a_press_in_one_update_are_all_the_fields_and_it_is_applied_once() {
    let mut rig = scene();
    let press = press_events(&rig, "button");
    rig.mapper.typed(TypedEntry::Char('c'));
    for event in &press {
        rig.mapper.pointer(*event);
    }
    rig.mapper.typed(TypedEntry::Char('d'));
    rig.feed(&[]);
    frame(&mut rig);
    assert_eq!(
        state(&mut rig),
        wanted("T=abcd", "none", "abcd", "wxyz", "1920", 1)
    );
}

#[test]
fn a_press_on_a_map_applies_the_field_and_a_press_on_the_field_itself_applies_nothing() {
    let mut rig = scene();
    let map = point(&rig, "map");
    click_at(&mut rig, map);
    assert_eq!(
        state(&mut rig),
        wanted("T=ab", "none", "ab", "wxyz", "1920", 0)
    );
    let mut rig = scene();
    click(&mut rig, "T");
    update(&mut rig, &typed("c"), &[]);
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "abc", "wxyz", "1920", 0),
        "a press on the field being edited tells nobody, and it goes on editing"
    );
}

#[test]
fn a_character_then_a_press_on_another_text_field_in_one_update_applies_the_first_with_it_and_the_second_edits_untouched()
 {
    let mut rig = scene();
    let press = press_events(&rig, "U");
    update(&mut rig, &typed("c"), &press);
    assert_eq!(
        state(&mut rig),
        wanted("T=abc", "U", "abc", "wxyz", "1920", 0),
        "the character was T's, T applied, and U edits with its text kept"
    );
    // Text input stayed on across the handoff, and what is typed now is U's: at the end of its text.
    update(&mut rig, &typed("!"), &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=abc", "U", "abc", "wxyz!", "1920", 0)
    );
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(calls, ["input true 1"], "text input was turned on once");
}

#[test]
fn enter_then_a_character_with_a_press_on_another_field_applies_without_the_character_and_it_reaches_no_field()
 {
    let mut rig = scene();
    let press = press_events(&rig, "U");
    update(
        &mut rig,
        &[TypedEntry::Enter, TypedEntry::Char('7')],
        &press,
    );
    assert_eq!(
        state(&mut rig),
        wanted("T=ab", "U", "ab", "wxyz", "1920", 0),
        "the Enter applied T; the character was nobody's; U edits"
    );
}

// ---- One editor for both kinds ----

#[test]
fn a_press_on_the_number_field_applies_the_text_field_and_a_press_on_a_text_field_applies_the_number_field()
 {
    let mut rig = scene();
    click(&mut rig, "N");
    assert_eq!(
        state(&mut rig),
        wanted("T=ab", "N", "ab", "wxyz", "1920", 0),
        "T applied and N edits"
    );
    // Letters mean nothing to a number field; its digits are its own.
    update(&mut rig, &typed("x7"), &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=ab", "N", "ab", "wxyz", "1920", 0),
        "a full number field takes no digit, and no letter"
    );
    update(&mut rig, &[TypedEntry::Backspace; 2], &[]);
    update(&mut rig, &typed("55"), &[]);
    click(&mut rig, "U");
    assert_eq!(
        state(&mut rig),
        wanted("T=ab,N=1955", "U", "ab", "wxyz", "1955", 0),
        "N applied and U edits"
    );
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(
        calls,
        ["input true 1"],
        "text input stayed on from one field to the next"
    );
}

// ---- Applying ----

#[test]
fn enter_tab_and_a_press_elsewhere_each_apply_the_text_once_and_an_unchanged_text_tells_nobody() {
    let mut rig = field_rig("", &["report()", "report()", "report()", "report()"]);
    start(&mut rig, "T");
    update(&mut rig, &typed("one"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=one", "none", "one", "wxyz", "1920", 0),
        "Enter"
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("!"), &[]);
    update(&mut rig, &[TypedEntry::Tab], &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=one,T=one!", "none", "one!", "wxyz", "1920", 0),
        "Tab applies and does nothing more"
    );
    start(&mut rig, "T");
    update(&mut rig, &[TypedEntry::Backspace], &[]);
    let button = point(&rig, "button");
    click_at(&mut rig, button);
    assert_eq!(
        state(&mut rig),
        wanted("T=one,T=one!,T=one", "none", "one", "wxyz", "1920", 1),
        "a press elsewhere, which still acts"
    );
    // The text the field already held: nobody is told.
    start(&mut rig, "T");
    update(
        &mut rig,
        &[TypedEntry::Backspace, TypedEntry::Char('e')],
        &[],
    );
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=one,T=one!,T=one", "none", "one", "wxyz", "1920", 1),
        "the same text again calls nobody"
    );
}

#[test]
fn an_empty_text_is_applied_as_the_caller_decides() {
    let mut rig = field_rig(
        r#"
function after_apply(name)
  if name == "U" and fields.U:get() == "" then fields.U:set("save-1") end
end
"#,
        &[
            "report()",
            r#"
eq(table.concat(applied, ","), "U=", "the caller was told of the empty text")
eq(fields.U:get(), "save-1", "and put back its own")
eq(shown("U"), "save-1")
expect(not fields.U:editing() and fields.U.caret == nil, "the editing was over when the caller's code ran")
"#,
            "report()",
        ],
    );
    start(&mut rig, "U");
    update(&mut rig, &[TypedEntry::Backspace; 6], &[]);
    assert_eq!(state(&mut rig), wanted("", "U", "", "", "1920", 0));
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened();
    assert_eq!(
        state(&mut rig),
        wanted("U=", "none", "", "save-1", "1920", 0)
    );
}

// ---- Escape ----

#[test]
fn escape_while_editing_puts_the_old_text_back_and_closes_nothing_and_the_next_escape_acts_as_before()
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
local f = fields.U
expect(not f:editing() and f.caret == nil, "the field does not edit")
eq(shown("U"), "wxyz", "the old text is back")
eq(buf("U"), "wxyz")
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
    start(&mut rig, "U");
    update(&mut rig, &[TypedEntry::Backspace; 3], &[]);
    update(&mut rig, &typed("77"), &[]);
    assert_eq!(state(&mut rig), wanted("", "U", "", "w77", "1920", 0));
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

#[test]
fn escape_while_the_number_field_edits_is_the_same_editing_and_restores_its_number() {
    let mut rig = field_rig(
        "",
        &[
            "report()",
            "report()",
            "expect(not W.closed, 'the window is open')",
        ],
    );
    start(&mut rig, "N");
    update(&mut rig, &[TypedEntry::Backspace; 2], &[]);
    assert_eq!(state(&mut rig), wanted("", "N", "", "wxyz", "19", 0));
    assert!(rig.escape(), "Escape declines");
    assert_eq!(state(&mut rig), wanted("", "none", "", "wxyz", "1920", 0));
    rig.opened();
}

#[test]
fn escape_with_no_field_editing_acts_as_before() {
    let mut rig = field_rig("", &["expect(W.closed, \"Escape closed the window\")"]);
    assert!(rig.escape());
    rig.opened();
}

// ---- A held key, the pause menu, the lifecycle ----

#[test]
fn three_backspace_entries_take_three_characters_however_they_arrive() {
    let mut rig = field_rig("", &["report()", "report()"]);
    start(&mut rig, "T");
    update(&mut rig, &typed("abcdefg"), &[]);
    update(&mut rig, &[TypedEntry::Backspace; 3], &[]);
    assert_eq!(state(&mut rig), wanted("", "T", "abcd", "wxyz", "1920", 0));
    // A held arrow and a held letter act again at each entry.
    update(&mut rig, &[TypedEntry::Left; 3], &[]);
    update(&mut rig, &[TypedEntry::Char('q'); 3], &[]);
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "aqqqbcd", "wxyz", "1920", 0)
    );
}

#[test]
fn under_the_pause_menu_a_3_typed_into_an_editing_text_field_adds_a_3_and_its_speed_key_sends_nothing()
 {
    let mut rig = field_rig("", &["menu.open(VIEW)", "report()"]);
    // The game's Digit3 is bound to the speed action, as the shipped configuration binds it.
    rig.mapper = Mapper::new(
        Bindings::from_entries(vec![("time_speed_3".to_owned(), vec!["Digit3".to_owned()])])
            .expect("bindings"),
    );
    start(&mut rig, "T");
    // With no menu the key is the game's: it sets the speed, and the field takes the character as well.
    rig.mapper.key(Key::Digit3, true, false);
    update(&mut rig, &typed("3"), &[]);
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
    update(&mut rig, &typed("3"), &[]);
    assert!(
        sent(&rig).is_empty(),
        "the speed action is stopped at the simulation"
    );
    assert_eq!(
        state(&mut rig),
        wanted("", "T", "33", "wxyz", "1920", 0),
        "and the field took the 3"
    );
}

#[test]
fn closing_the_window_while_a_text_field_edits_applies_nothing_and_leaves_no_caret_key_or_editing_state()
 {
    let mut rig = field_rig(
        "",
        &[
            "W:close()",
            r#"
eq(#applied, 0, "nothing was applied")
expect(numberfield.current() == nil, "no field edits")
expect(typing.editing() == nil, "the typing function has no editor")
expect(fields.T.caret == nil and fields.T.frame == nil, "no caret, and the field is gone")
eq(count_hits("textfield_"), 0, "no hit key is left")
fields.T:destroy()
"#,
        ],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("abc"), &[]);
    rig.opened();
    assert!(
        notes(&rig).contains(&"input false 2".to_owned()),
        "text input went off: {:?}",
        notes(&rig)
    );
    rig.opened();
    // What is typed now reaches no field, and a press where the field was is not a press on it.
    update(&mut rig, &typed("5"), &[]);
    click_at(&mut rig, (60.0, 60.0));
}

#[test]
fn an_apply_callback_that_closes_the_window_or_restarts_editing_meets_a_coherent_state() {
    // The caller closes the window in its callback: no editor is left and text input is off.
    let mut rig = field_rig(
        "function after_apply(name) if name == \"T\" then W:close() end end",
        &[r#"
expect(numberfield.current() == nil, "no field edits")
expect(typing.editing() == nil, "the typing function has no editor")
eq(table.concat(applied, ","), "T=ab", "T's callback ran once")
expect(W.closed, "the window closed")
"#],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("ab"), &[]);
    click(&mut rig, "U");
    rig.opened();
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("input "))
        .collect();
    assert_eq!(calls, ["input true 1", "input false 2"]);
    // The caller restarts the same field's editing in its callback: it edits, with the caret at the end of the text it applied.
    let mut rig = field_rig(
        r#"
function after_apply(name)
  if name == "T" and not restarted then
    restarted = true
    fields.T:set(fields.T:get() .. "!")
  end
end
"#,
        &["report()"],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("ab"), &[]);
    update(&mut rig, &[TypedEntry::Enter], &[]);
    assert_eq!(
        state(&mut rig),
        wanted("T=ab", "none", "ab!", "wxyz", "1920", 0),
        "a `set` in the callback met a finished editing and set the text"
    );
}

#[test]
fn arming_a_capture_while_a_text_field_edits_applies_it_once_with_its_text_and_asks_for_the_capture_after()
 {
    let mut rig = field_rig(
        r#"
request_log = {}
function arm(id)
  typing.capture(id, function(asked)
    request_log[#request_log + 1] = string.format("%s applied=%s current=%s editor=%s", asked, table.concat(applied, ","),
      tostring(numberfield.current() ~= nil), tostring(typing.editing() ~= nil))
  end)
end
"#,
        &[r#"
arm("cap")
eq(#request_log, 1, "the capture request was made once")
eq(request_log[1], "cap applied=T=abc current=false editor=false",
  "the field was applied (with its text) and ended before the request was made")
expect(not fields.T:editing() and fields.T.caret == nil, "no editing state or caret is left")
eq(fields.T.frame.line, theme_lib.kind("window").outline, "and the outline is back")
eq(shown("T"), "abc")
"#],
    );
    start(&mut rig, "T");
    update(&mut rig, &typed("abc"), &[]);
    rig.opened();
    assert!(
        notes(&rig).contains(&"input false 2".to_owned()),
        "text input went off: {:?}",
        notes(&rig)
    );
}

#[test]
fn a_press_on_a_text_field_while_a_capture_is_armed_starts_nothing() {
    let mut rig = field_rig(
        "typing.capture('cap', function(id) eq(id, 'cap') end)",
        &[r#"
expect(typing.editing() == nil, "capture owns the keyboard")
expect(numberfield.current() == nil and not fields.T:editing(), "capture refused starting a field")
expect(fields.T.caret == nil, "no editing caret during capture")
eq(fields.T.frame.line, fields.T.colours.outline, "no hot editing outline during capture")
eq(#applied, 0)
"#],
    );
    click(&mut rig, "T");
    rig.opened();
    assert!(!notes(&rig).iter().any(|x| x.starts_with("input true ")));
}
