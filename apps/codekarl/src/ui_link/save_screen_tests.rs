//! The save / load screen (CK-305, `scripts/ui/lib/save_screen.lua`) in a real UI VM, in the script test rig, with no window and no GPU.
//!
//! The shipped menu, window, card stack, text field, list bar and button libraries run over the shipped font and theme, through the real
//! input path: Rust feeds pointer events and Escape, the script's frame hook dispatches them. Most tests answer the game's save calls with a
//! stand-in in Lua (`STORE`, `CALLS`, `REFUSE`) so that every entry, every time and every refusal is exact; the last tests run the real
//! simulation core over a temporary saves directory. Nothing sleeps.

use ck_window::{PointerEvent, TypedEntry};

use super::group_tests::Rig;
use super::listbar_tests::{click_at, frame, point};
use super::menu_tests::{click_marked, menu_rig};
use super::numberfield_tests::{start, update};
use super::widget_witness_tests::points;

/// What every test adds: the stand-in game calls, the screen's marks (the fixed buttons, every card, the chosen card's buttons and the open
/// question's), and readers for what the cards show.
const SAVES: &str = r#"
save_lib = require("lib.save_screen")
function now_utc(offset) return os.date("!%Y-%m-%dT%H:%M:%SZ", os.time() + offset) end
SCENARIO = "ireland_768"
STORE, CALLS, REFUSE, NEXT = {}, {}, {}, 0
-- An entry of the game's list. An extra set to `false` leaves the field out.
function entry(filename, name, extra)
  local e = {filename = filename, name = name, fit = "fits", this_build = "yes", written = now_utc(-3600), date = "3 Mar 771"}
  for key, value in pairs(extra or {}) do e[key] = value end
  for key, value in pairs(e) do if value == false then e[key] = nil end end
  return e
end
function add(...) for _, e in ipairs({...}) do STORE[#STORE + 1] = e end end
local function copy(list)
  local out = {}
  for index, e in ipairs(list) do
    out[index] = {}
    for key, value in pairs(e) do out[index][key] = value end
  end
  return out
end
local function find(filename)
  for index, e in ipairs(STORE) do if e.filename == filename then return index, e end end
end
function called(text) for _, call in ipairs(CALLS) do if call == text then return true end end return false end
function calls() return table.concat(CALLS, "|") end
game.scenario = function() return SCENARIO end
game.saves = function()
  CALLS[#CALLS + 1] = "saves"
  if REFUSE.saves then return {ok = false, reason = REFUSE.saves} end
  table.sort(STORE, function(a, b) return a.filename < b.filename end)
  return {ok = true, entries = copy(STORE)}
end
game.save = function(name)
  CALLS[#CALLS + 1] = "save " .. name
  if REFUSE.save then return {ok = false, reason = REFUSE.save} end
  NEXT = NEXT + 1
  add(entry(string.format("zz-new-%d.cksave", NEXT), name, {written = now_utc(10 * NEXT)}))
  return {ok = true}
end
game.save_over = function(filename)
  CALLS[#CALLS + 1] = "over " .. filename
  if REFUSE.over then return {ok = false, reason = REFUSE.over} end
  NEXT = NEXT + 1
  local _, e = find(filename)
  e.written = now_utc(10 * NEXT)
  return {ok = true}
end
game.delete_save = function(filename)
  CALLS[#CALLS + 1] = "delete " .. filename
  if REFUSE.delete then return {ok = false, reason = REFUSE.delete} end
  table.remove(STORE, (find(filename)))
  return {ok = true}
end
game.load_save = function(filename)
  CALLS[#CALLS + 1] = "load " .. filename
  if REFUSE.load then return {ok = false, reason = REFUSE.load} end
  return {ok = true}
end
game.load_save_guarded = function(filename) CALLS[#CALLS + 1] = "guarded " .. filename end
game.on_save_result = function(fn) RESULT_FN = fn end
-- The game's answer to a guarded load, delivered as the host delivers it.
function answer(filename, result) RESULT_FN(filename, result) end

"#;

/// The screen's marks and readers (shared by the stand-in rigs and the real-core rig).
const SCREEN_MARKS: &str = r#"
function the_screen() return save_lib.current() end
function open_screen() menu_button("save / load").on_click() end
function mark_screen()
  local s = the_screen()
  if s == nil or s.window.closed then return end
  local ox, oy = s.window:content_origin()
  local function mid(name, b)
    local w, h = b:size()
    mark(name, ox + b.x + w / 2, oy + b.y + h / 2)
  end
  mid("new_save", s.new_button)
  mid("field", s.field)
  mid("filter", s.bar.filter_button)
  mid("sort", s.bar.sort_button)
  local wx, wy = s.window:position()
  local cb = s.window.close_button
  local cw, ch = cb:size()
  mark("x", wx + cb.x + cw / 2, wy + cb.y + ch / 2)
  for key in pairs(s.cards.item_keys) do
    local index = tonumber(key:match("_(%d+)$"))
    local x, y, w = hit_rect(key)
    mark("card" .. index, x + w / 2, y + 6)
  end
  for _, view in pairs(s.cards.views) do
    for label, b in pairs(view.buttons or {}) do
      local x, y, w, h = hit_rect(b.key)
      mark("btn_" .. label, x + w / 2, y + h / 2)
    end
  end
  if s.question ~= nil then
    for label, b in pairs(s.question.buttons) do
      local x, y, w, h = hit_rect(b.key)
      mark("q_" .. label, x + w / 2, y + h / 2)
    end
  end
end
-- The file names of the cards in the stack's order.
function order()
  local out = {}
  for index, item in ipairs(the_screen().cards.items) do out[index] = item.filename end
  return table.concat(out, ",")
end
-- What card `index` shows: its name, the text at its left on the second line, the text at its right, and their colours.
function card_lines(index)
  local s = the_screen()
  local top, height = s.cards.tops[index], s.cards.items[index].height * P
  local found = {}
  for _, line in ipairs(lines_in(s.cards.group)) do
    if line.y >= top and line.y < top + height then found[#found + 1] = line end
  end
  table.sort(found, function(a, b) if a.y ~= b.y then return a.y < b.y end return a.x < b.x end)
  local out = {name = line_text(found[1]), name_colour = found[1].colour}
  for i = 2, #found do
    if found[i].x == 3 * P then
      out.left, out.left_colour = line_text(found[i]), found[i].colour
    else
      out.right = line_text(found[i])
    end
  end
  return out
end
function card_frame(index) return the_screen().cards.views[index].frame end
function field_text() return the_screen().field:get() end
function count_text() return line_text(the_screen().bar.count) end
function refusal_text() local t = line_text(the_screen().refusal_line) return t ~= "" and t or nil end
"#;

/// A rig of the menu with the stand-in game calls, then `extra`, then `steps` (one per `opened()`).
fn save_rig(extra: &str, steps: &[&str]) -> Rig {
    menu_rig(&format!("{SAVES}\n{SCREEN_MARKS}\n{extra}"), steps)
}

fn press(rig: &mut Rig, name: &str) {
    click_marked(rig, name);
}

// ---- Opening and closing ----

#[test]
fn the_save_load_button_opens_one_screen_above_the_backdrop_and_x_and_escape_go_back_to_the_menu() {
    let mut rig = save_rig(
        "add(entry('a.cksave', 'alpha'))\nmenu.open(VIEW)\nmark_buttons()",
        &[
            // 1
            r#"
local b = menu_button("save / load")
expect(not b.inactive and not dim_look(b), "save / load is live")
expect(the_screen() == nil, "no screen yet")
open_screen()
local s = the_screen()
expect(s ~= nil and not s.window.closed, "the button opened the screen")
local w = s.window
eq(w.title, "Save / Load") eq(w.kind, "window")
expect(not w.draggable and not w.fixed and w.close_button ~= nil, "not draggable, with its close x")
local width, height = w:size()
eq(width, 400 * P, "400 UI pixels wide") eq(height, 300 * P, "and 300 high")
local x, y = w:position()
expect(math.abs(x + width / 2 - 400) <= P and math.abs(y + height / 2 - 300) <= P, "centred in the view")
expect(menu.is_open(), "the menu is still open under it")
eq(windows.top(), w, "the screen is the top window")
eq(count_hits("window_frame_"), 2, "the menu and the screen")
local content_width = w.content_width
local field_width, field_height = s.field:size()
local new_width = s.new_button:size()
eq(s.status_line.y, 0, "the saved status is at the top")
eq(line_text(s.status_line), "saved")
eq(s.field.y, 12 * P, "the name field shifts down to make room")
eq(field_width, content_width - new_width - 4 * P, "the name field is as wide as the content less the button and a 4-pixel gap")
eq(field_height, 15 * P) eq(s.new_button.x, content_width - new_width) eq(s.new_button.y, 12 * P)
local rule = scene.quads[s.rule]
eq(rule.y, 29 * P, "a line under them") eq(rule.h, P) eq(rule.w, content_width)
eq(rule.tint, theme_lib.kind("window").outline, "in the kind's outline")
eq(s.cards.y, 32 * P, "the cards start under the line")
eq(s.cards.y + s.cards.height + 19 * P, w.content_height, "and the list bar has the bottom 19 pixels")
menu_button("save / load").on_click()
eq(the_screen().window, w, "a second press raises the same screen")
eq(count_hits("window_frame_"), 2, "at most one")
mark_screen()
"#,
            // 2
            r#"
expect(the_screen() == nil, "x closed the screen")
expect(menu.is_open(), "back to the menu, still open")
expect(menu_button("save / load") ~= nil)
open_screen()
expect(the_screen() ~= nil, "it opens again")
"#,
            // 3
            r#"
expect(the_screen() == nil, "Escape closed the screen")
expect(menu.is_open(), "and left the menu")
"#,
        ],
    );
    rig.opened();
    let x = point(&rig, "x");
    click_at(&mut rig, x);
    rig.opened();
    assert!(rig.escape(), "Escape closes the screen and declines");
    rig.opened();
    assert!(
        rig.controls.keys_blocked(),
        "the game's keys stay blocked while the menu is open"
    );
    assert!(rig.escape(), "the next Escape closes the menu");
    assert!(!rig.controls.keys_blocked());
}

// ---- Cards ----

/// Local noon `days_ago` days back as a UTC RFC 3339 time, and the date and clock the screen should show for it.
const NOON: &str = r#"
function noon(days_ago, hour)
  local t = os.date("*t")
  t.hour, t.min, t.sec = hour or 12, 0, 0
  local epoch = os.time(t) - days_ago * 86400
  return os.date("!%Y-%m-%dT%H:%M:%SZ", epoch), os.date("*t", epoch)
end
MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
"#;

#[test]
fn a_card_shows_the_name_the_in_game_date_and_the_time_written_in_local_time() {
    let mut rig = save_rig(
        &format!(
            r#"{NOON}
local today = noon(0)
local yesterday = noon(1)
local older, when = noon(5)
OLDER = string.format("%d %s %d", when.day, MONTHS[when.month], when.year)
add(entry("a.cksave", "alpha", {{written = today}}),
  entry("b.cksave", "bravo", {{written = yesterday, date = "28 Feb 771"}}),
  entry("c.cksave", "charlie", {{written = older, date = "1 Jan 770"}}),
  entry("d.cksave", "delta", {{written = false}}),
  entry("e.cksave", "echo", {{written = noon(0, 11), date = false}}),
  entry("quicksave.cksave", "quicksave", {{written = noon(0, 10)}}))
menu.open(VIEW)
mark_buttons()
"#
        ),
        &[r#"
open_screen()
eq(order(), "a.cksave,e.cksave,quicksave.cksave,b.cksave,c.cksave,d.cksave", "newest written first, none written last")
local alpha = card_lines(1)
eq(alpha.name, "alpha") eq(alpha.left, "3 Mar 771") eq(alpha.right, "today 12:00", "today, local time")
eq(alpha.name_colour, theme_lib.colour("text"), "the name in text") eq(alpha.left_colour, theme_lib.colour("dim"), "the date in dim")
local echo = card_lines(2)
eq(echo.name, "echo") eq(echo.left, nil, "a save with no in-game date leaves that part empty") eq(echo.right, "today 11:00")
local quick = card_lines(3)
eq(quick.name, "quicksave", "named by its file name when the header has none") eq(quick.right, "today 10:00")
local bravo = card_lines(4)
eq(bravo.left, "28 Feb 771") eq(bravo.right, "yesterday 12:00")
eq(card_lines(5).right, OLDER, "an older save shows D Mon YYYY")
local delta = card_lines(6)
eq(delta.name, "delta") eq(delta.left, "3 Mar 771") eq(delta.right, nil, "a save with no time written shows none")
local kind = theme_lib.kind("window")
eq(card_frame(1).fill, theme_lib.colour("body"), "body fill") eq(card_frame(1).line, kind.outline, "outline")
eq(card_frame(1).cut, 3, "a cut-3 frame")
local cards = the_screen().cards
eq(cards.tops[2] - cards.tops[1], 28 * P + 3 * P, "3 UI pixels between cards")
"#],
    );
    rig.opened();
}

#[test]
fn a_card_that_cannot_load_is_greyed_with_its_reason_and_a_click_chooses_one_card_at_a_time() {
    let mut rig = save_rig(
        r#"
add(entry("a.cksave", "alpha"),
  entry("b.cksave", "bravo", {fit = "version", this_build = "no"}),
  entry("c.cksave", "charlie", {fit = "content", this_build = "no"}),
  entry("d.cksave", "delta", {fit = "unreadable", this_build = "unknown", written = false, date = false}))
menu.open(VIEW)
mark_buttons()
"#,
        &[
            // 1: the filter hides the three
            r#"
open_screen()
eq(order(), "a.cksave", "opens showing this version only")
the_screen().bar:clear()
eq(order(), "a.cksave,b.cksave,c.cksave,d.cksave", "all shows the greyed saves (alphabetical: none has a time but alpha's)")
local kind = theme_lib.kind("window")
eq(card_frame(1).line, kind.outline)
for index, reason in ipairs({"made by another version: cannot load", "made with other game data: cannot load", "cannot be read"}) do
  local lines = card_lines(index + 1)
  eq(lines.name_colour, theme_lib.colour("dim"), "a greyed card's name is dim")
  eq(lines.left, reason)
  eq(card_frame(index + 1).line, theme_lib.colour("track"), "its outline is track")
end
eq(card_lines(4).name, "delta", "an unreadable save is named by its file name or its header's name")
for index = 1, 4 do expect(the_screen().cards.views[index].buttons == nil or next(the_screen().cards.views[index].buttons) == nil, "no buttons yet") end
eq(the_screen().chosen, nil, "nothing is chosen")
mark_screen()
"#,
            // 2: chose alpha
            r#"
local s = the_screen()
eq(s.chosen, "a.cksave", "a click chose it")
eq(card_frame(1).line, theme_lib.kind("window").hot, "its outline is hot")
local labels = {}
for label in pairs(s.cards.views[1].buttons) do labels[#labels + 1] = label end
table.sort(labels)
eq(table.concat(labels, ","), "delete,load,save", "load, save and delete")
eq(s.cards.items[1].height, 47, "the chosen card is taller")
mark_screen()
"#,
            // 3: chose bravo (another version)
            r#"
local s = the_screen()
eq(s.chosen, "b.cksave", "another card moves the choice")
eq(card_frame(1).line, theme_lib.kind("window").outline, "the first is no longer hot")
eq(card_frame(2).line, theme_lib.kind("window").hot, "the greyed card is hot too")
local labels = {}
for label in pairs(s.cards.views[2].buttons) do labels[#labels + 1] = label end
eq(table.concat(labels, ","), "delete", "only delete for a save that cannot load")
mark_screen()
"#,
            // 4: clicked bravo again
            r#"
eq(the_screen().chosen, "b.cksave", "clicking the chosen card again keeps it chosen")
eq(calls(), "saves", "nothing but the listing has been asked of the game")
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    press(&mut rig, "card2");
    rig.opened(); // 3
    press(&mut rig, "card2");
    rig.opened(); // 4
}

// ---- The list bar ----

#[test]
fn the_list_bar_starts_with_this_version_ticked_and_sorts_with_saves_lacking_a_value_last() {
    let mut rig = save_rig(
        r#"
add(entry("a.cksave", "alpha", {written = now_utc(-100)}),
  entry("b.cksave", "bravo", {written = now_utc(-300), date = "28 Feb 771"}),
  entry("c.cksave", "charlie", {written = now_utc(-200), date = "1 Jan 770"}),
  entry("d.cksave", "delta", {written = now_utc(-400), date = false}),
  entry("e.cksave", "echo", {fit = "version", this_build = "no", written = false, date = "9 Sep 769"}),
  entry("f.cksave", "foxtrot", {fit = "unreadable", this_build = "unknown", written = false, date = false}),
  entry("g.cksave", "golf", {fit = "content", this_build = "no", written = now_utc(-500), date = "1 Jan 771"}))
menu.open(VIEW)
mark_buttons()
"#,
        &[r#"
open_screen()
local bar = the_screen().bar
expect(bar.ticked.version["this version"] == true and bar.ticks == 1, "this version starts ticked")
expect(bar.filter_button.selected, "the filter button shows a tick")
eq(count_text(), "4 of 7", "the count")
eq(order(), "a.cksave,c.cksave,b.cksave,d.cksave", "this version's saves, newest written first")
bar:clear()
eq(count_text(), "", "no box ticked: no count") eq(order(), "a.cksave,c.cksave,b.cksave,d.cksave,g.cksave,e.cksave,f.cksave", "all shows every save, greyed ones too")
bar:toggle(1, "other versions")
eq(order(), "g.cksave,e.cksave,f.cksave", "a save whose build is unknown counts as another version")
bar:clear()
bar:choose(1)
eq(order(), "a.cksave,b.cksave,c.cksave,d.cksave,e.cksave,f.cksave,g.cksave", "name, ascending first")
bar:choose(1)
eq(order(), "g.cksave,f.cksave,e.cksave,d.cksave,c.cksave,b.cksave,a.cksave", "a second press turns it round")
bar:choose(2)
eq(order(), "a.cksave,b.cksave,g.cksave,c.cksave,e.cksave,d.cksave,f.cksave", "in-game date, latest first, saves with no date last")
bar:choose(2)
eq(order(), "e.cksave,c.cksave,g.cksave,b.cksave,a.cksave,d.cksave,f.cksave", "turned round: no date still last")
bar:choose(3)
eq(order(), "a.cksave,c.cksave,b.cksave,d.cksave,g.cksave,e.cksave,f.cksave", "written, newest first, saves with no time last")
bar:choose(nil)
eq(order(), "a.cksave,c.cksave,b.cksave,d.cksave,g.cksave,e.cksave,f.cksave", "the default is newest written first")
"#],
    );
    rig.opened();
}

// ---- New save and the names ----

#[test]
fn new_save_makes_a_new_save_unless_a_listed_save_that_fits_has_the_name() {
    let mut rig = save_rig(
        r#"
add(entry("a.cksave", "alpha"),
  entry("b.cksave", "twin"), entry("c.cksave", "twin"),
  entry("d.cksave", "ghost", {fit = "version", this_build = "no"}),
  entry("e.cksave", "hidden", {this_build = "no"}))
save_lib.set_time({year = 771, month = 3, day = 3})
menu.open(VIEW)
mark_buttons()
function try(name)
  CALLS = {}
  the_screen().field:set(name)
  mark_screen()
end
function question_title() local s = the_screen() return s.question ~= nil and s.question.window.title or nil end
"#,
        &[
            r#"open_screen() try("fresh")"#,
            // 2: no save of the name: a new save was made
            r#"
eq(calls(), "save fresh|saves", "no save of that name: a new save, no question")
eq(question_title(), nil)
try("alpha")
"#,
            // 3: one that fits: asks
            r#"
eq(calls(), "", "nothing was sent yet") eq(question_title(), "Overwrite this save?", "it asks")
mark_screen()
"#,
            // 4: overwrite replaces that file
            r#"
eq(calls(), "over a.cksave|saves", "overwrite saves over that file, and the list is read again")
eq(question_title(), nil)
try("twin")
"#,
            // 5: two fit: the first by file name
            r#"
eq(question_title(), "Overwrite this save?")
mark_screen()
"#,
            r#"
eq(calls(), "over b.cksave|saves", "only the first file by name is replaced, not c.cksave")
try("ghost")
"#,
            // 7: one that cannot load: passed over
            r#"
eq(calls(), "save ghost|saves", "a save of that name that cannot load is passed over: a new save")
local count = 0
for _, e in ipairs(STORE) do if e.name == "ghost" then count = count + 1 end end
eq(count, 2, "two cards then show one name")
try("  alpha  ")
"#,
            // 8: spaces at its ends
            r#"
eq(question_title(), "Overwrite this save?", "spaces at the ends are taken off: it asks")
mark_screen()
"#,
            // 9: cancel
            r#"
eq(calls(), "over a.cksave|saves")
try("Alpha")
"#,
            // 10: a capital differs
            r#"
eq(calls(), "save Alpha|saves", "one that differs by a capital is a new save")
eq(question_title(), nil)
try("hidden")
"#,
            // 11: hidden by the filter but compared
            r#"
eq(question_title(), "Overwrite this save?", "a save the filter hides is compared too")
local s = the_screen()
for _, e in ipairs(s.entries) do if e.name == "hidden" then eq(e.this_build, "no") end end
mark_screen()
"#,
            r#"
eq(calls(), "", "cancel sends nothing")
eq(question_title(), nil)
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "new_save");
    rig.opened(); // 2
    press(&mut rig, "new_save");
    rig.opened(); // 3
    press(&mut rig, "q_overwrite");
    rig.opened(); // 4
    press(&mut rig, "new_save");
    rig.opened(); // 5
    press(&mut rig, "q_overwrite");
    rig.opened(); // 6
    press(&mut rig, "new_save");
    rig.opened(); // 7
    press(&mut rig, "new_save");
    rig.opened(); // 8
    press(&mut rig, "q_overwrite");
    rig.opened(); // 9
    press(&mut rig, "new_save");
    rig.opened(); // 10
    press(&mut rig, "new_save");
    rig.opened(); // 11
    press(&mut rig, "q_cancel");
    rig.opened(); // 12
}

// ---- Asking first ----

/// The question's title and its lines in reading order, and its button labels.
const QUESTION: &str = r#"
function question_lines()
  local s = the_screen()
  local found = {}
  for _, line in ipairs(lines_in(s.question.window.content)) do
    if s.question.buttons[line_text(line)] == nil then found[#found + 1] = line end
  end
  table.sort(found, function(a, b) return a.y < b.y end)
  local out = {}
  for index, line in ipairs(found) do out[index] = line_text(line) end
  return table.concat(out, " / ")
end
function question_buttons()
  local labels = {}
  for label in pairs(the_screen().question.buttons) do labels[#labels + 1] = label end
  table.sort(labels)
  return table.concat(labels, ",")
end
function has_question() return the_screen().question ~= nil end
"#;

#[test]
fn save_and_delete_ask_first_and_cancel_and_escape_send_nothing_while_the_screen_behind_does_not_respond()
 {
    let mut rig = save_rig(
        &format!(
            "{QUESTION}\nadd(entry('a.cksave', 'alpha', {{written = now_utc(-100)}}), entry('b.cksave', 'bravo', {{written = now_utc(-200)}}))\nmenu.open(VIEW)\nmark_buttons()"
        ),
        &[
            r"open_screen() mark_screen() CALLS = {}",
            // 2: card alpha chosen
            r#"eq(the_screen().chosen, "a.cksave") mark_screen()"#,
            // 3: "save" asked
            r#"
local s = the_screen()
expect(has_question(), "save asks")
eq(s.question.window.title, "Overwrite this save?")
eq(question_lines(), '"alpha" will be replaced / by the game as it is now.')
eq(question_buttons(), "cancel,overwrite")
eq(calls(), "", "nothing is sent yet")
eq(windows.top(), s.question.window, "the question is above the screen")
mark_screen()
"#,
            // 4: pressed the card behind, "x" behind
            r#"
expect(has_question(), "still open") eq(the_screen().chosen, "a.cksave", "the card behind did not respond")
mark_screen()
"#,
            r#"
expect(has_question(), "x behind did not respond") expect(not the_screen().window.closed)
mark_screen()
"#,
            // 6: cancel
            r#"
expect(not has_question(), "cancel closed the question") eq(calls(), "", "and sent nothing")
expect(not the_screen().window.closed, "the screen is still open")
mark_screen()
"#,
            // 7: asks again, then Escape (done in Rust before this step)
            r#"
expect(has_question(), "save asks again")
"#,
            r#"
expect(not has_question(), "Escape is cancel") eq(calls(), "", "and sends nothing")
expect(not the_screen().window.closed, "and the screen is open: Escape closed the question only")
mark_screen()
"#,
            // 9: overwrite
            r"expect(has_question()) mark_screen()",
            r#"
eq(calls(), "over a.cksave|saves", "overwrite saved over that file and listed again")
expect(not has_question())
eq(the_screen().chosen, nil, "nothing is chosen after a save")
eq(order(), "a.cksave,b.cksave", "the saved one is first in the default order")
CALLS = {}
mark_screen()
"#,
            // 11: choose bravo, delete asks
            r#"eq(the_screen().chosen, "b.cksave") mark_screen()"#,
            r#"
local s = the_screen()
eq(s.question.window.title, "Delete this save?")
eq(question_lines(), '"bravo" will be deleted. / This cannot be undone.')
eq(question_buttons(), "cancel,delete")
eq(calls(), "", "delete asked first")
mark_screen()
"#,
            r#"
expect(not has_question()) eq(calls(), "", "cancel sent nothing")
eq(the_screen().chosen, "b.cksave", "the choice stays")
mark_screen()
"#,
            r#"expect(has_question(), "delete asks again") mark_screen()"#,
            r#"
eq(calls(), "delete b.cksave|saves", "delete removes that file and lists again")
eq(order(), "a.cksave", "its card goes")
eq(the_screen().chosen, nil, "and nothing is chosen")
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    press(&mut rig, "btn_save");
    rig.opened(); // 3
    press(&mut rig, "card2");
    rig.opened(); // 4
    press(&mut rig, "x");
    rig.opened(); // 5
    press(&mut rig, "q_cancel");
    rig.opened(); // 6
    press(&mut rig, "btn_save");
    rig.opened(); // 7
    assert!(rig.escape(), "Escape closes the question and declines");
    rig.opened(); // 8
    press(&mut rig, "btn_save");
    rig.opened(); // 9
    press(&mut rig, "q_overwrite");
    rig.opened(); // 10
    press(&mut rig, "card2");
    rig.opened(); // 11
    press(&mut rig, "btn_delete");
    rig.opened(); // 12
    press(&mut rig, "q_cancel");
    rig.opened(); // 13
    press(&mut rig, "btn_delete");
    rig.opened(); // 14
    press(&mut rig, "q_delete");
    rig.opened(); // 15
}

// ---- Loading ----

const LOAD_SAVES: &str = "add(entry('a.cksave', 'alpha', {written = now_utc(-100)}), entry('b.cksave', 'bravo', {written = now_utc(-200)}))\nmenu.open(VIEW)\nmark_buttons()";

#[test]
fn every_press_of_load_sends_one_guarded_load_and_the_games_unsaved_answer_opens_the_question() {
    let mut rig = save_rig(
        &format!("{QUESTION}\n{LOAD_SAVES}"),
        &[
            r"open_screen() mark_screen() CALLS = {}",
            // 2: chose alpha
            r"mark_screen()",
            // 3: load pressed: one guarded load, no plain load
            r#"
eq(calls(), "guarded a.cksave", "a press of load sends the guarded load and nothing else")
expect(the_screen().busy ~= nil, "and waits for the answer")
expect(not has_question(), "nothing is asked yet")
answer("a.cksave", {ok = false, reason = "unsaved"})
local s = the_screen()
expect(has_question(), "answered unsaved, the question opens")
eq(s.question.window.title, "Load this save?")
eq(question_lines(), "The game has unsaved changes. / They will be discarded.")
eq(question_buttons(), "cancel,load")
eq(refusal_text(), nil, "that answer is not a refusal and shows no line")
expect(s.busy == nil, "the screen responds again")
eq(calls(), "guarded a.cksave", "no plain load yet")
mark_screen()
"#,
            // 4: cancel
            r#"
expect(not has_question()) eq(calls(), "guarded a.cksave", "cancel sends nothing")
expect(not the_screen().window.closed and menu.is_open(), "the screen and the menu stay")
mark_screen()
"#,
            // 5: pressed load again, after a game_time came in
            r#"
eq(calls(), "guarded a.cksave|guarded a.cksave", "each press sends one guarded load")
answer("a.cksave", {ok = false, reason = "unsaved"})
expect(has_question(), "the question opens again")
mark_screen()
"#,
            // 6: Escape (in Rust) cancelled it
            r#"
expect(not has_question(), "Escape is cancel") eq(calls(), "guarded a.cksave|guarded a.cksave", "and sends nothing")
mark_screen()
"#,
            // 7: pressed load a third time
            r#"
answer("a.cksave", {ok = false, reason = "unsaved"})
mark_screen()
"#,
            // 8: the question's load: the plain load for the same file
            r#"
eq(calls(), "guarded a.cksave|guarded a.cksave|guarded a.cksave|load a.cksave", "the question's load sends the plain load for the same file")
expect(the_screen() == nil, "the screen closed")
expect(not menu.is_open(), "and the menu")
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    press(&mut rig, "btn_load");
    rig.opened(); // 3
    press(&mut rig, "q_cancel");
    rig.opened(); // 4
    // A `game_time` arrives between the presses: the screen asks nothing of it.
    rig.deliver_game_time(&crate::readout_tests::snapshot(3, 2, None));
    frame(&mut rig); // the helper's frames are another size: the view is 800 by 600 again, the window where it was
    press(&mut rig, "btn_load");
    rig.opened(); // 5
    assert!(rig.escape(), "Escape cancels the question");
    rig.opened(); // 6
    press(&mut rig, "btn_load");
    rig.opened(); // 7
    press(&mut rig, "q_load");
    rig.opened(); // 8
    assert!(
        !rig.controls.keys_blocked(),
        "the menu closed: the game's keys are unblocked"
    );
}

#[test]
fn a_guarded_load_answered_with_success_closes_the_screen_and_the_menu_with_no_question() {
    let mut rig = save_rig(
        LOAD_SAVES,
        &[
            r"open_screen() mark_screen() CALLS = {}",
            r"mark_screen()",
            r#"
eq(calls(), "guarded a.cksave")
answer("a.cksave", {ok = true})
expect(the_screen() == nil, "the screen closed")
expect(not menu.is_open(), "and the menu, as return does")
eq(calls(), "guarded a.cksave", "nothing was asked and no plain load was sent")
eq(count_hits("window_frame_"), 0, "no window is left")
"#,
        ],
    );
    rig.opened();
    press(&mut rig, "card1");
    rig.opened();
    assert!(rig.controls.keys_blocked());
    press(&mut rig, "btn_load");
    rig.opened();
    assert!(!rig.controls.keys_blocked(), "the keys are unblocked");
}

#[test]
fn another_refusal_of_either_load_shows_the_refusal_line_and_the_screen_stays() {
    let mut rig = save_rig(
        &format!("{QUESTION}\n{LOAD_SAVES}"),
        &[
            r"open_screen() mark_screen()",
            r"mark_screen()",
            r#"
answer("a.cksave", {ok = false, reason = "the save is damaged"})
expect(not has_question(), "no question for another refusal")
eq(refusal_text(), "load failed: the save is damaged", "the line says what failed")
expect(the_screen().busy == nil and not the_screen().window.closed and menu.is_open(), "the screen stays")
eq(the_screen().refusal_line.test_runs[1].colour, theme_lib.colour("over_limit"), "in the over_limit colour")
mark_screen()
"#,
            // 4: pressed load again: the line goes with the next action; unsaved this time, and the plain load refused
            r#"
eq(refusal_text(), nil, "the next action takes the line away")
REFUSE.load = "no such file"
answer("a.cksave", {ok = false, reason = "unsaved"})
mark_screen()
"#,
            r#"
eq(refusal_text(), "load failed: no such file", "the plain load's refusal is shown too")
expect(not has_question() and not the_screen().window.closed, "the screen stays")
"#,
        ],
    );
    rig.opened();
    press(&mut rig, "card1");
    rig.opened();
    press(&mut rig, "btn_load");
    rig.opened();
    press(&mut rig, "btn_load");
    rig.opened();
    press(&mut rig, "q_load");
    rig.opened();
}

// ---- Until a load is answered ----

#[test]
fn from_a_load_press_until_its_answer_nothing_on_the_screen_responds_x_and_escape_included() {
    let mut rig = save_rig(
        LOAD_SAVES,
        &[
            r"open_screen() mark_screen()",
            r"CALLS = {} mark_screen()",
            // 3: load pressed: busy; marks for the presses that follow
            r#"
eq(calls(), "guarded a.cksave")
expect(the_screen().busy ~= nil)
mark_screen()
"#,
            // 4: after the presses and Escape
            r#"
eq(calls(), "guarded a.cksave", "a second press of load, a press on another card, on new save and on x sent nothing")
local s = the_screen()
expect(s ~= nil and not s.window.closed, "x and Escape did not close the screen") expect(menu.is_open())
eq(s.chosen, "a.cksave", "the choice did not move")
expect(s.question == nil and s.busy ~= nil, "still waiting")
answer("a.cksave", {ok = false, reason = "other"})
expect(s.busy == nil, "the answer ended the wait")
eq(refusal_text(), "load failed: other")
answer("a.cksave", {ok = false, reason = "again"})
eq(refusal_text(), "load failed: other", "a second answer, with nothing awaited, is ignored")
mark_screen()
"#,
            // 5: the screen responds again
            r#"
eq(the_screen().chosen, "b.cksave", "a press on another card works again")
mark_screen()
"#,
            r#"
eq(calls(), "guarded a.cksave|save ireland 768|saves", "new save works again (a new save under the generated name, then listed)")
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    press(&mut rig, "btn_load");
    rig.opened(); // 3
    for name in ["btn_load", "card2", "new_save", "x", "filter", "field"] {
        let at = point(&rig, name);
        let key = rig.key_at(at.0, at.1).expect("a key under the point");
        assert!(
            key.starts_with("save_shield_"),
            "{name}: the shield takes the press, not {key}"
        );
        click_at(&mut rig, at);
    }
    assert!(
        rig.escape(),
        "Escape declines and closes nothing while a load is unanswered"
    );
    rig.opened(); // 4
    press(&mut rig, "card2");
    rig.opened(); // 5
    press(&mut rig, "new_save");
    rig.opened(); // 6
}

// ---- Refusals ----

#[test]
fn a_refused_save_or_delete_keeps_the_screen_and_says_what_failed_until_the_next_action() {
    let mut rig = save_rig(
        &format!(
            "{QUESTION}\nadd(entry('a.cksave', 'alpha', {{written = now_utc(-100)}}))\nsave_lib.set_time({{year = 771, month = 3, day = 3}})\nmenu.open(VIEW)\nmark_buttons()"
        ),
        &[
            r#"open_screen() the_screen().field:set("mine") REFUSE.save = "disk full" mark_screen()"#,
            // 2: new save refused
            r#"
eq(refusal_text(), "save failed: disk full", "the line says what failed")
eq(the_screen().refusal_line.test_runs[1].colour, theme_lib.colour("over_limit"), "in over_limit")
eq(field_text(), "mine", "the field keeps the name")
eq(order(), "a.cksave", "the list is as it was")
eq(the_screen().cards_top, 45 * P, "the cards make room for both lines")
REFUSE.save = nil
mark_screen()
"#,
            // 3: new save works; the line goes
            r#"
eq(refusal_text(), nil, "the next action takes the line away")
eq(the_screen().cards_top, 32 * P, "the cards come back up")
eq(order(), "zz-new-1.cksave,a.cksave", "the saved one is first in the default order")
expect(field_text() ~= "mine" and field_text() ~= "", "the name field got a fresh generated name")
eq(#STORE, 2)
mark_screen()
"#,
            // 4: choose the new one, delete it, refused
            r#"eq(the_screen().chosen, "zz-new-1.cksave") REFUSE.delete = "in use" mark_screen()"#,
            r"expect(has_question()) mark_screen()",
            r#"
eq(refusal_text(), "delete failed: in use", "a refused delete says so")
eq(order(), "zz-new-1.cksave,a.cksave", "its card stays")
REFUSE.delete = nil
REFUSE.over = "read only"
mark_screen()
"#,
            // 7: save over, refused
            r"expect(has_question()) mark_screen()",
            r#"
eq(refusal_text(), "save failed: read only")
eq(order(), "zz-new-1.cksave,a.cksave")
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "new_save");
    rig.opened(); // 2
    press(&mut rig, "new_save");
    rig.opened(); // 3
    press(&mut rig, "card1");
    rig.opened(); // 4
    press(&mut rig, "btn_delete");
    rig.opened(); // 5
    press(&mut rig, "q_delete");
    rig.opened(); // 6
    press(&mut rig, "btn_save");
    rig.opened(); // 7
    press(&mut rig, "q_overwrite");
    rig.opened(); // 8
}

#[test]
fn a_listing_that_fails_shows_the_refusal_and_no_cards() {
    let mut rig = save_rig(
        "add(entry('a.cksave', 'alpha'))\nREFUSE.saves = 'no saves directory'\nmenu.open(VIEW)\nmark_buttons()",
        &[r#"
open_screen()
eq(refusal_text(), "cannot list the saves: no saves directory")
eq(order(), "", "no cards")
expect(not the_screen().window.closed, "the screen opens all the same")
"#],
    );
    rig.opened();
}

// ---- The two library changes ----

#[test]
fn a_card_view_that_raises_its_hits_puts_its_buttons_above_the_cards_own_region_and_other_items_are_unchanged()
 {
    let mut rig = menu_rig(
        r#"
cards_lib = require("lib.cards")
new_window(200, 120)
clicked = {}
raising = button.new -- keeps the library's own function in view for the test below
local function item(label, raise)
  return {height = 30, on_click = function() clicked[#clicked + 1] = "card " .. label end, draw = function(group, x, y)
    local b = button.new{x = x + 4 * P, y = y + 6 * P, text = label, group = group, owner = W, on_click = function() clicked[#clicked + 1] = "button " .. label end}
    BUTTONS[label] = b
    local view = {destroy = function() b:destroy() end}
    if raise then view.raise_hits = function() b:raise_hit() end end
    return view
  end}
end
BUTTONS = {}
stack = cards_lib.new{window = W, x = 0, y = 0, width = 200 * P, height = 120 * P, gap = 3, items = {item("raised", true), item("plain", false)}}
function mark_items()
  for label, b in pairs(BUTTONS) do
    local x, y, w, h = hit_rect(b.key)
    mark("button_" .. label, x + w / 2, y + h / 2)
    mark("key_" .. label, 0, 0)
  end
  for key in pairs(stack.item_keys) do
    local x, y, w = hit_rect(key)
    mark("card_" .. key:match("_(%d+)$"), x + w - 4, y + 4)
  end
end
"#,
        &[
            r"mark_items()",
            r#"eq(table.concat(clicked, ","), "button raised", "the raising view's button took the press")
mark_items()"#,
            r#"eq(table.concat(clicked, ","), "button raised,card plain", "a view with no raise_hits is as before: the card's region is above its button")
mark_items()"#,
            r#"eq(table.concat(clicked, ","), "button raised,card plain,card raised", "the card's own region still takes a press beside the button")
stack:set_pos(0)
stack:set_items({item("raised", true), item("plain", false)}, 3)
mark_items()"#,
            r#"eq(table.concat(clicked, ","), "button raised,card plain,card raised,button raised", "after the stack is drawn again the button is still above")"#,
        ],
    );
    rig.opened();
    press(&mut rig, "button_raised");
    rig.opened();
    press(&mut rig, "button_plain");
    rig.opened();
    press(&mut rig, "card_1");
    rig.opened();
    press(&mut rig, "button_raised");
    rig.opened();
}

#[test]
fn a_window_with_keep_open_ignores_escape_while_it_says_so_and_closes_when_it_does_not() {
    let mut rig = menu_rig(
        r#"
KEEP = true
K = windows.new{title = "keeper", kind = "window", modal = false, content_width = 100 * P, content_height = 40 * P, keep_open = function() return KEEP end}
other = windows.new{title = "ordinary", kind = "window", content_width = 100 * P, content_height = 40 * P, x = 300, y = 300}
windows.top():raise()
K:raise()
"#,
        &[
            r#"eq(windows.top(), K, "the keeper is the top window") expect(not K.closed)"#,
            r#"expect(not K.closed, "Escape did nothing while keep_open says so") eq(windows.top(), K)
KEEP = false"#,
            r#"expect(K.closed, "Escape closed it once it does not keep itself open") eq(windows.top(), other, "and the next window is the top")"#,
            r#"expect(other.closed, "an ordinary window (no keep_open) closes as before")"#,
        ],
    );
    rig.opened();
    assert!(rig.escape(), "Escape declines");
    rig.opened();
    assert!(rig.escape(), "Escape declines");
    rig.opened();
    assert!(rig.escape(), "Escape declines");
    rig.opened();
}

// ---- The name field ----

#[test]
fn an_applied_empty_name_puts_the_generated_one_back_and_typed_text_is_kept() {
    let mut rig = save_rig(
        r#"
typing_lib = require("lib.typing")
local real_set_text_input = window.set_text_input
window.set_text_input = function(on)
  local generation = real_set_text_input(on)
  note("input " .. tostring(on) .. " " .. generation)
  return generation
end
local base_frame = on_frame
function on_frame(update) typing_lib.dispatch(update) base_frame(update) end
add(entry("a.cksave", "alpha"))
save_lib.set_time({year = 771, month = 3, day = 3})
menu.open(VIEW)
mark_buttons()
"#,
        &[
            r#"open_screen() GENERATED = field_text() expect(GENERATED ~= "", "the field opens with a generated name") mark_screen()"#,
            r#"expect(the_screen().field:editing(), "a press on the field edits it") eq(field_text(), GENERATED)"#,
            r#"eq(the_screen().field.position, 0, "everything was taken away") expect(the_screen().field:editing())"#,
            r#"
expect(not the_screen().field:editing(), "Enter applied the empty text")
eq(field_text(), GENERATED, "an applied empty name puts the generated one back")
mark_screen()
"#,
            r"expect(the_screen().field:editing())",
            r#"
expect(not the_screen().field:editing())
eq(field_text(), "mine", "typed text is kept")
"#,
        ],
    );
    rig.opened(); // 1
    start(&mut rig, "field");
    rig.opened(); // 2
    update(&mut rig, &[TypedEntry::Backspace; 48], &[]);
    rig.opened(); // 3
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened(); // 4
    start(&mut rig, "field");
    update(&mut rig, &[TypedEntry::Backspace; 48], &[]);
    for entry in "mine".chars() {
        update(&mut rig, &[TypedEntry::Char(entry)], &[]);
    }
    rig.opened(); // 5
    update(&mut rig, &[TypedEntry::Enter], &[]);
    rig.opened(); // 6
}

// ---- The real core over a temporary saves directory ----

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use ck_sim::{Date, Simulation};

use crate::game::core::{Core, Message, Outputs, Rebuild, SaveCall};
use crate::game::owner::{self, SimOwner};
use crate::game::saves::Identity;
use crate::game::slot::SimSlot;
use crate::game::tests::{Faults, World, apply_nothing, scheduler, start as game_start};
use crate::testing::Dir;

fn identity(build: &str) -> Identity {
    Identity {
        build: build.to_owned(),
        scenario: "test".to_owned(),
        scenario_hash: "aa".to_owned(),
        map_hash: "bb".to_owned(),
        scripts_hash: "cc".to_owned(),
        rules_hash: "dd".to_owned(),
        start: Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 0,
        },
    }
}

fn core(build: &str, saves: PathBuf, slot: &Arc<SimSlot>) -> Core<World, crate::game::GameCommand> {
    let publishing = Arc::clone(slot);
    let failing = Arc::clone(slot);
    Core::new(
        Simulation::new(
            game_start(),
            42,
            World::default(),
            scheduler(Faults::default()),
            apply_nothing,
        ),
        identity(build),
        saves,
        Rebuild {
            scheduler: Box::new(|| scheduler(Faults::default())),
            reset: Box::new(|_| {}),
            apply: apply_nothing,
        },
        Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(move |snapshot| publishing.publish(snapshot)),
            fail: Box::new(move |message| failing.fail(message)),
        },
    )
}

/// A real simulation core on its own thread over a temporary saves directory, answering what the rig's `game` calls send. Every message
/// passes a forwarding thread that logs it first (`log`), so a test sees exactly what the script sent.
struct RealSaves {
    _owner: SimOwner,
    dir: Dir,
    log: Arc<Mutex<Vec<String>>>,
}

impl RealSaves {
    fn attach(rig: &mut Rig) -> Self {
        let dir = Dir::new();
        let saves = dir.0.join("saves");
        let (sender, receiver) = owner::channel();
        let slot = Arc::clone(&rig.simulation_slot);
        let thread_slot = Arc::clone(&slot);
        let thread_saves = saves.clone();
        let owner = SimOwner::spawn(sender.clone(), receiver, slot, move || {
            core("this-build", thread_saves, &thread_slot)
        })
        .expect("the simulation thread starts");
        let log = Arc::new(Mutex::new(Vec::new()));
        let incoming = std::mem::replace(&mut rig.control_receiver, mpsc::channel().1);
        let forwarding = Arc::clone(&log);
        std::thread::spawn(move || {
            for message in incoming {
                let line = match &message {
                    Message::Control(control) => format!("control {control:?}"),
                    Message::MarkUnsaved => "mark_unsaved".to_owned(),
                    Message::Save(call, _) => format!("save {call:?}"),
                    Message::GuardedLoad { filename, .. } => format!("guarded {filename}"),
                    Message::Stop => "stop".to_owned(),
                    Message::Cancel => "cancel".to_owned(),
                };
                forwarding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(line);
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            _owner: owner,
            dir,
            log,
        }
    }

    fn saves(&self) -> PathBuf {
        self.dir.0.join("saves")
    }

    /// The messages sent so far, without the menu's own pause.
    fn sent(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|line| !line.starts_with("control "))
            .cloned()
            .collect()
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.saves())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Writes a save of another build into `saves`, as an older game would have left it.
fn write_foreign_save(saves: &Path, name: &str) {
    let slot = SimSlot::new(Arc::new(|| {}));
    let mut other = core("another-build", saves.to_path_buf(), &slot);
    let (reply, answer) = mpsc::channel();
    other.dispatch(
        Message::Save(SaveCall::New(name.to_owned()), reply),
        Duration::ZERO,
    );
    assert!(
        answer.recv().is_ok(),
        "the other build's core answered the save"
    );
}

/// The Lua that notes each answer to a guarded load as it is delivered (`result FILE OK REASON`), so that Rust can wait for it.
const NOTE_RESULTS: &str = r#"
local real_on_save_result = game.on_save_result
game.on_save_result = function(fn)
  if fn == nil then return real_on_save_result(nil) end
  return real_on_save_result(function(filename, result)
    note("result " .. filename .. " " .. tostring(result.ok) .. " " .. tostring(result.reason))
    fn(filename, result)
  end)
end
"#;

/// A rig over a real core: the stand-in game calls of `SAVES` are not installed; `game.scenario` alone is stood in for.
fn real_rig(extra: &str, steps: &[&str]) -> (Rig, RealSaves) {
    let body = format!(
        r#"
save_lib = require("lib.save_screen")
game.scenario = function() return "ireland_768" end
{SCREEN_MARKS}
{NOTE_RESULTS}
{extra}
"#
    );
    let mut rig = menu_rig(&body, steps);
    let real = RealSaves::attach(&mut rig);
    (rig, real)
}

/// Frames until the script has noted `needle`, at most a few thousand: the core answers on its own thread.
fn wait_for_note(rig: &mut Rig, needle: &str) {
    for _ in 0..5000 {
        if super::listbar_tests::notes(rig)
            .iter()
            .any(|line| line.starts_with(needle))
        {
            return;
        }
        frame(rig);
        std::thread::yield_now();
    }
    panic!("the script never noted {needle}");
}

#[test]
fn the_screen_tracks_the_real_core_saved_flag_through_marks_saves_and_loads() {
    let (mut rig, _real) = real_rig(
        r#"
game.on_time(function(time)
  save_lib.set_time(time)
  save_status_events = (save_status_events or 0) + 1
  note("save status " .. tostring(time.unsaved) .. " " .. save_status_events)
end)
menu.open(VIEW)
mark_buttons()
function expect_status(label, colour)
  local found
  for _, line in ipairs(lines_in(the_screen().window.content)) do
    if line_text(line) == label then found = line end
  end
  expect(found ~= nil, "the screen shows " .. label)
  eq(found.colour, theme_lib.colour(colour), "the " .. label .. " word has its theme colour")
end
"#,
        &[
            r#"
open_screen()
expect_status("saved", "saved")
game.mark_unsaved()
mark_screen()
"#,
            r#"
expect_status("unsaved", "dim")
the_screen().field:set("one")
mark_screen()
"#,
            r#"
expect_status("saved", "saved")
eq(order(), "one.cksave", "the successful save is listed")
game.mark_unsaved()
mark_screen()
"#,
            r#"
expect_status("unsaved", "dim")
local failed = game.save("")
expect(not failed.ok, "the real core refuses an empty save name")
expect_status("unsaved", "dim")
mark_screen()
"#,
            r#"
expect_status("unsaved", "dim")
mark_screen()
"#,
            r#"
expect(the_screen().question ~= nil, "the guarded load answers unsaved")
expect_status("unsaved", "dim")
mark_screen()
"#,
            r#"
expect(the_screen() == nil and not menu.is_open(), "the confirmed load closes the screen as before")
menu.open(VIEW)
mark_buttons()
open_screen()
expect_status("saved", "saved")
"#,
        ],
    );

    wait_for_note(&mut rig, "save status false 1");
    rig.opened(); // saved at open

    wait_for_note(&mut rig, "save status true 2");
    rig.opened(); // the mark reaches the open screen

    press(&mut rig, "new_save");
    wait_for_note(&mut rig, "save status false 3");
    rig.opened(); // a successful save clears the label

    wait_for_note(&mut rig, "save status true 4");
    rig.opened(); // the second mark makes it unsaved again

    press(&mut rig, "card1");
    rig.opened(); // select the saved game
    press(&mut rig, "btn_load");
    wait_for_note(&mut rig, "result one.cksave false unsaved");
    rig.opened(); // guarded-load refusal agrees with the label

    press(&mut rig, "q_load");
    wait_for_note(&mut rig, "save status false 5");
    rig.opened(); // the plain load succeeds and reopening reads saved
}

#[test]
fn new_save_save_over_and_delete_work_on_real_files_in_a_temporary_saves_directory() {
    let (mut rig, real) = real_rig(
        "menu.open(VIEW)\nmark_buttons()",
        &[
            r#"
open_screen()
eq(order(), "", "no saves yet: a saves directory that does not exist is an empty list")
the_screen().field:set("first")
mark_screen()
"#,
            r#"
eq(order(), "first.cksave", "the new save is listed")
eq(the_screen().rows[1].entry.name, "first")
eq(the_screen().rows[1].entry.fit, "fits")
eq(the_screen().rows[1].entry.this_build, "yes")
expect(the_screen().rows[1].entry.written ~= nil and the_screen().rows[1].entry.date == "1 Jan 768", "its header has the time written and the in-game date")
expect(field_text() ~= "first", "the name field got a fresh name")
mark_screen()
"#,
            r"mark_screen()",
            r#"expect(the_screen().question ~= nil, "save asks") mark_screen()"#,
            r#"
expect(the_screen().question == nil)
eq(order(), "first.cksave", "still one save: the same file")
mark_screen()
"#,
            r"mark_screen()",
            r#"expect(the_screen().question ~= nil, "delete asks") mark_screen()"#,
            r#"eq(order(), "", "the save is gone")"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "new_save");
    rig.opened(); // 2
    assert_eq!(real.files(), ["first.cksave"], "one file on disk");
    press(&mut rig, "card1");
    rig.opened(); // 3
    press(&mut rig, "btn_save");
    rig.opened(); // 4
    press(&mut rig, "q_overwrite");
    rig.opened(); // 5
    assert_eq!(real.files(), ["first.cksave"], "the same file");
    press(&mut rig, "card1");
    rig.opened(); // 6
    press(&mut rig, "btn_delete");
    rig.opened(); // 7
    press(&mut rig, "q_delete");
    rig.opened(); // 8
    assert!(real.files().is_empty(), "the file is deleted");
    assert_eq!(
        real.sent(),
        [
            "save List",
            "save New(\"first\")",
            "save List",
            "save Over(\"first.cksave\")",
            "save List",
            "save Delete(\"first.cksave\")",
            "save List",
        ],
        "what the script sent the core, in order"
    );
}

#[test]
fn a_press_of_load_asks_the_real_core_which_answers_success_or_unsaved() {
    let (mut rig, real) = real_rig(
        "menu.open(VIEW)\nmark_buttons()",
        &[
            r#"open_screen() the_screen().field:set("one") mark_screen()"#,
            r"mark_screen()",
            r"mark_screen()",
            // 4: after the first load: the clean game loaded at once
            r#"
expect(the_screen() == nil and not menu.is_open(), "a game with no unsaved changes loads at once, the screen and the menu closing")
menu.open(VIEW)
mark_buttons()
open_screen()
game.mark_unsaved()
mark_screen()
"#,
            r"mark_screen()",
            // 6: the second press: unsaved
            r#"
local s = the_screen()
expect(s ~= nil and s.question ~= nil, "unsaved changes: the question opens")
eq(s.question.window.title, "Load this save?")
eq(refusal_text(), nil, "no refusal line")
mark_screen()
"#,
            r#"expect(the_screen() == nil and not menu.is_open(), "the question's load loaded the game")"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "new_save");
    rig.opened(); // 2
    press(&mut rig, "card1");
    rig.opened(); // 3
    press(&mut rig, "btn_load");
    wait_for_note(&mut rig, "result one.cksave true");
    rig.opened(); // 4
    press(&mut rig, "card1");
    rig.opened(); // 5
    press(&mut rig, "btn_load");
    wait_for_note(&mut rig, "result one.cksave false unsaved");
    rig.opened(); // 6
    press(&mut rig, "q_load");
    rig.opened(); // 7
    assert!(!rig.controls.keys_blocked(), "the keys are unblocked");
    assert_eq!(
        real.sent(),
        [
            "save List",
            "save New(\"one\")",
            "save List",
            "guarded one.cksave",
            "save List",
            "mark_unsaved",
            "guarded one.cksave",
            "save Load(\"one.cksave\")",
        ],
        "one guarded load for each press, and one plain load, from the question"
    );
}

#[test]
fn a_save_of_another_build_is_listed_greyed_under_other_versions_and_can_be_deleted() {
    let (mut rig, real) = real_rig(
        "menu.open(VIEW)\nmark_buttons()",
        &[
            r#"
open_screen()
eq(order(), "", "the filter starts on this version: the other build's save is hidden")
eq(count_text(), "0 of 1")
the_screen().bar:toggle(1, "other versions")
eq(order(), "elsewhere.cksave", "other versions shows it")
local lines = card_lines(1)
eq(lines.name, "elsewhere") eq(lines.left, "made by another version: cannot load")
eq(card_frame(1).line, theme_lib.colour("track"))
mark_screen()
"#,
            r"mark_screen()",
            r"expect(the_screen().question ~= nil) mark_screen()",
            r#"eq(order(), "")"#,
        ],
    );
    write_foreign_save(&real.saves(), "elsewhere");
    assert_eq!(real.files(), ["elsewhere.cksave"]);
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    press(&mut rig, "btn_delete");
    rig.opened(); // 3
    press(&mut rig, "q_delete");
    rig.opened(); // 4
    assert!(real.files().is_empty());
}

#[test]
fn the_screen_and_an_open_question_are_centred_again_when_the_view_changes() {
    let mut rig = save_rig(
        &format!("{QUESTION}\n{LOAD_SAVES}"),
        &[
            r"open_screen() mark_screen()",
            r"mark_screen()",
            r"
local s = the_screen()
local w = s.window
local width, height = w:size()
local x, y = w:position()
expect(math.abs(x + width / 2 - 500) <= P and math.abs(y + height / 2 - 350) <= P, 'the screen is centred in the new view')
local qw = s.question.window
local qwidth, qheight = qw:size()
local qx, qy = qw:position()
expect(math.abs(qx + qwidth / 2 - 500) <= P and math.abs(qy + qheight / 2 - 350) <= P, 'and so is the question')
local bx, by, bw, bh = hit_rect(s.question.backdrop.key)
eq(bw, 1000) eq(bh, 700)
",
        ],
    );
    rig.opened();
    press(&mut rig, "card1");
    rig.opened();
    press(&mut rig, "btn_save");
    rig.deliver_fixture_frame_at(1000, 700, 2);
    rig.opened();
}

// ---- The generated name ----

#[test]
fn the_generated_name_is_the_scenario_and_the_date_regenerated_on_opening_and_after_a_save_never_over_48_characters()
 {
    let mut rig = save_rig(
        "save_lib.set_time({year = 771, month = 3, day = 3})\nmenu.open(VIEW)\nmark_buttons()",
        &[
            r#"
open_screen()
eq(field_text(), "ireland 768, 3 Mar 771", "the scenario's id with _ as spaces, a comma and the in-game date as the game shows it")
save_lib.set_time({year = 771, month = 3, day = 4})
eq(field_text(), "ireland 768, 3 Mar 771", "the field is not touched when the date changes")
the_screen().field:set("ireland 768, 3 Mar 771")
mark_screen()
"#,
            r#"
eq(calls(), "saves|save ireland 768, 3 Mar 771|saves", "new save saved under the field's name")
eq(field_text(), "ireland 768, 4 Mar 771", "a fresh generated name after the save")
save_lib.close()
save_lib.set_time({year = 772, month = 12, day = 25})
open_screen()
eq(field_text(), "ireland 768, 25 Dec 772", "generated again whenever the screen opens")
-- the 48-character budget
local function name_for(id, time)
  save_lib.close()
  SCENARIO = id
  save_lib.set_time(time)
  open_screen()
  return field_text()
end
local long = string.rep("a", 48)
local name = name_for(long, {year = 771, month = 3, day = 3})
eq(name, string.rep("a", 37) .. ", 3 Mar 771", "an id of 48 characters in the year 771: 37 of it, the comma and the date")
eq(#name, 48, "48 characters")
name = name_for(long, {year = 9999, month = 12, day = 31})
eq(name, string.rep("a", 35) .. ", 31 Dec 9999", "the date is kept whole, at most 11 characters")
eq(#name, 48)
name = name_for(string.rep("a", 36) .. "_" .. string.rep("b", 11), {year = 771, month = 3, day = 3})
eq(name, string.rep("a", 36) .. ", 3 Mar 771", "a cut part that ends in a space loses it: no space before the comma")
eq(#name, 47)
name = name_for("ireland_768", {year = 771, month = 3, day = 3})
eq(name, "ireland 768, 3 Mar 771", "a short id is whole")
eq(save_lib.generated_name("ireland_768", nil), "ireland 768", "with no date known yet, the scenario's part alone")
eq(save_lib.generated_name(string.rep("a", 48), nil), string.rep("a", 48))
"#,
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "new_save");
    rig.opened(); // 2
}

// ---- Later events of the same update ----

/// The Lua that lets the typing library run and notes when text input is turned on, as the name field's tests need.
const TYPING: &str = r#"
typing_lib = require("lib.typing")
local real_set_text_input = window.set_text_input
window.set_text_input = function(on)
  local generation = real_set_text_input(on)
  note("input " .. tostring(on) .. " " .. generation)
  return generation
end
local base_frame = on_frame
function on_frame(update) typing_lib.dispatch(update) base_frame(update) end
"#;

/// Presses and releases each named mark in turn, all in ONE input update, as a fast player's clicks arrive together.
fn click_together(rig: &mut Rig, names: &[&str]) {
    let mut events = Vec::new();
    for name in names {
        let (x, y) = point(rig, name);
        events.push(PointerEvent::Down(points(x, y)));
        events.push(PointerEvent::Up(points(x, y)));
    }
    rig.feed(&events);
    frame(rig);
}

#[test]
fn a_press_on_the_name_field_in_the_same_update_as_load_starts_no_editor_and_typing_changes_nothing()
 {
    let mut rig = save_rig(
        &format!("{TYPING}\n{LOAD_SAVES}\nsave_lib.set_time({{year = 771, month = 3, day = 3}})"),
        &[
            r"open_screen() GENERATED = field_text() mark_screen()",
            r"mark_screen() CALLS = {}",
            r"
eq(calls(), 'guarded a.cksave', 'the load was sent, once, before the later events')
local s = the_screen()
expect(s.busy ~= nil, 'and the screen waits')
expect(not s.field:editing(), 'the field press that followed in the same update started no editor')
eq(field_text(), GENERATED)
",
            r"
expect(not the_screen().field:editing(), 'no editor while the load is unanswered')
eq(field_text(), GENERATED, 'typed text changed nothing')
eq(calls(), 'guarded a.cksave')
answer('a.cksave', {ok = false, reason = 'other'})
mark_screen()
",
            r"expect(the_screen().field:editing(), 'once the answer has come a press on the field edits it again')",
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    click_together(&mut rig, &["btn_load", "field"]);
    rig.opened(); // 3
    update(&mut rig, &[TypedEntry::Char('x')], &[]);
    rig.opened(); // 4
    start(&mut rig, "field");
    rig.opened(); // 5
}

#[test]
fn a_press_on_x_in_the_same_update_as_load_does_not_overtake_it() {
    let mut rig = save_rig(
        LOAD_SAVES,
        &[
            r"open_screen() mark_screen()",
            r"mark_screen() CALLS = {}",
            r"
eq(calls(), 'guarded a.cksave', 'one guarded load is sent: the later close did not overtake it')
local s = the_screen()
expect(s ~= nil and not s.window.closed and s.busy ~= nil, 'and the screen is open and waiting')
expect(menu.is_open())
answer('a.cksave', {ok = false, reason = 'other'})
mark_screen()
",
            r"expect(the_screen() == nil, 'x closes the screen once the answer has come')",
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    click_together(&mut rig, &["btn_load", "x"]);
    rig.opened(); // 3
    press(&mut rig, "x");
    rig.opened(); // 4
}

#[test]
fn the_other_buttons_hold_a_later_press_of_the_same_update_until_their_action_has_run() {
    let mut rig = save_rig(
        &format!(
            "{TYPING}\n{QUESTION}\n{LOAD_SAVES}\nsave_lib.set_time({{year = 771, month = 3, day = 3}})"
        ),
        &[
            r"open_screen() mark_screen()",
            r"mark_screen()",
            r"
local s = the_screen()
expect(s.question ~= nil, 'save asks')
expect(not s.field:editing(), 'a field press in the same update started no editor behind the question')
mark_screen()
",
            r"expect(the_screen().question == nil) mark_screen()",
            r"
local s = the_screen()
expect(s ~= nil and not s.window.closed, 'x in the same update as new save closed nothing')
eq(calls(), 'saves|save ireland 768, 3 Mar 771|saves', 'the new save was made')
expect(s.question == nil)
",
        ],
    );
    rig.opened(); // 1
    press(&mut rig, "card1");
    rig.opened(); // 2
    click_together(&mut rig, &["btn_save", "field"]);
    rig.opened(); // 3
    press(&mut rig, "q_cancel");
    rig.opened(); // 4
    click_together(&mut rig, &["new_save", "x"]);
    rig.opened(); // 5
}

#[test]
fn the_pointer_gate_swallows_keyed_events_while_it_says_so_and_changes_nothing_when_unset() {
    let mut rig = menu_rig(
        r"
new_window(120, 60)
clicks = 0
go = button.new{x = 0, y = 0, text = 'go', group = W.content, owner = W, on_click = function() clicks = clicks + 1 end}
function mark_go()
  local x, y, w, h = hit_rect(go.key)
  mark('go', x + w / 2, y + h / 2)
end
",
        &[
            r"mark_go() eq(clicks, 0)",
            r"eq(clicks, 1, 'with no gate a click reaches its button') pointer_lib.set_gate(function(key) return key == go.key end)",
            r"eq(clicks, 1, 'a gate that returns true for the key swallows the click') pointer_lib.set_gate(function() return false end)",
            r"eq(clicks, 2, 'a gate that returns false changes nothing') pointer_lib.set_gate(nil)",
            r"eq(clicks, 3, 'with the gate cleared clicks reach the button again')",
        ],
    );
    rig.opened();
    for _ in 0..4 {
        press(&mut rig, "go");
        rig.opened();
    }
}
