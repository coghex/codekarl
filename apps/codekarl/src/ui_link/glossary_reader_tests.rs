//! The glossary reader (CK-300, `scripts/ui/lib/glossary_reader.lua`) in a real UI VM, in the script test rig, with no window and no GPU.
//!
//! The shipped menu, window, list, scroll bar and tooltip libraries run over the shipped glossary, font and theme, through the real
//! input path: Rust feeds pointer events and Escape, the script's frame hook dispatches them. A test reads what the script drew from the
//! scene shadow of `listbar_tests`'s prelude. Tests of the markup and of a click that scrolls the list use a stand-in `glossary`
//! (the reader and the tooltips both read the global at call time) so that the entries are known. Nothing sleeps.

use super::group_tests::Rig;
use super::listbar_tests::{click_at, frame, hover, point};
use super::menu_tests::{click_marked, menu_rig, sent};
use crate::game::controls::Control;

/// What every test adds: the menu opened, marks for the menu's buttons, and `mark_reader()`, which marks the reader's close button, the
/// rows that exist and the first rectangle of each link. `use_stub()` replaces the glossary with entries the tests know.
const READER: &str = r#"
reader_lib = require("lib.glossary_reader")
menu.open(VIEW)
mark_buttons()
function the_reader() return reader_lib.current() end
function mark_reader()
  local r = the_reader()
  if r == nil then return end
  local x, y = r.window:position()
  local b = r.window.close_button
  local w, h = b:size()
  mark("reader_close", x + b.x + w / 2, y + b.y + h / 2)
  for index, key in pairs(r.list.row_keys) do
    local rx, ry, rw, rh = hit_rect(key)
    mark("row" .. index, rx + rw / 2, ry + rh / 2)
  end
  if r.page ~= nil then
    for number, key in ipairs(r.page.keys) do
      local lx, ly, lw, lh = hit_rect(key)
      mark("link" .. number, lx + lw / 2, ly + lh / 2)
    end
  end
end
function use_stub()
  local entries = {
    alpha = {title = "Alpha", text = "Plain {{brace}} and a {beta|linked words} then {gamma} end."},
    beta = {title = "Beta", text = "About beta, see {alpha}."},
    gamma = {title = "Gamma", text = "Gamma text."},
    delta = {title = "Delta", text = string.rep("word ", 60)},
  }
  for i = 1, 30 do
    entries[string.format("filler_%02d", i)] = {title = string.format("Filler %02d", i), text = "Filler."}
  end
  local keys = {}
  for key in pairs(entries) do keys[#keys + 1] = key end
  table.sort(keys)
  glossary = {
    keys = function() return keys end,
    entry = function(key)
      local e = entries[key]
      if e == nil then return nil end
      return {key = key, title = e.title, text = e.text}
    end,
  }
end
-- The page's text lines as {text, colour, x, y} in UI pixels from the right column's top-left, in reading order.
function page_text()
  local r = the_reader()
  local found = {}
  for _, line in ipairs(lines_in(r.page.group)) do
    found[#found + 1] = {text = line_text(line), colour = line.colour, x = line.x / P, y = line.y / P, width = line.width}
  end
  table.sort(found, function(a, b) if a.y ~= b.y then return a.y < b.y end return a.x < b.x end)
  return found
end
"#;

fn reader_rig(extra: &str, steps: &[&str]) -> Rig {
    let mut all = vec![];
    all.extend_from_slice(steps);
    menu_rig(&format!("{READER}\n{extra}"), &all)
}

/// Opens the reader as a press on the menu's "glossary" button does, and runs the next step.
fn open_reader(rig: &mut Rig) {
    click_marked(rig, "glossary");
}

fn press(rig: &mut Rig, name: &str) {
    click_at(rig, point(rig, name));
}

// ---- Opening and closing ----

#[test]
fn the_glossary_button_opens_one_reader_above_the_backdrop_and_x_and_escape_go_back_to_the_menu() {
    let mut rig = reader_rig(
        "use_stub()",
        &[
            // 1
            r#"
local b = menu_button("glossary")
expect(not b.inactive and not dim_look(b), "the glossary button is live")
expect(the_reader() == nil, "no reader yet")
"#,
            // 2
            r#"
local r = the_reader()
expect(r ~= nil and not r.window.closed, "the button opened the reader")
local w = r.window
eq(w.title, "Glossary") eq(w.kind, "window")
expect(not w.draggable and not w.fixed and w.close_button ~= nil, "not draggable, with its close x")
local width, height = w:size()
eq(width, 400 * P, "400 UI pixels wide") eq(height, 300 * P, "and 300 high")
local x, y = w:position()
expect(math.abs(x + width / 2 - 400) <= P and math.abs(y + height / 2 - 300) <= P, "centred in the view")
expect(menu.is_open(), "the menu is still open under it")
eq(windows.top(), w, "the reader is the top window, as settings is")
eq(count_hits("window_frame_"), 2, "the menu and the reader")
menu_button("glossary").on_click()
eq(the_reader().window, w, "a second press raises the same reader")
eq(count_hits("window_frame_"), 2, "at most one reader")
mark_reader()
"#,
            // 2b
            "mark_reader()",
            // 3
            r#"
expect(the_reader() ~= nil, "still open")
eq(count_hits("tooltip_panel_"), 1, "a linked term's tooltip is open")
"#,
            // 4
            r#"
expect(the_reader() ~= nil and menu.is_open(), "Escape closed the tooltip first, not the reader")
eq(count_hits("tooltip_panel_"), 0)
"#,
            // 5
            r#"
expect(the_reader() == nil, "the next Escape closed the reader")
expect(menu.is_open(), "back to the menu, which is still open")
"#,
            // 6
            r#"
expect(the_reader() ~= nil, "opened again")
mark_reader()
"#,
            // 7
            r#"
expect(the_reader() == nil and menu.is_open(), "the close x went back to the menu")
"#,
            // 8
            r#"
expect(not menu.is_open(), "Escape in the menu closes the menu")
"#,
        ],
    );
    rig.opened(); // 1
    open_reader(&mut rig);
    rig.opened(); // 2
    // The reader is above the backdrop: a press on its list row reaches the list, not the backdrop.
    let row = point(&rig, "row1");
    let key = rig
        .key_at(row.0, row.1)
        .expect("a hit region over the first row");
    assert!(key.starts_with("list_"), "{key}");
    assert!(
        rig.controls.keys_blocked(),
        "the game keys stay blocked under the menu and the reader"
    );
    // A tooltip opened over a linked term (the backdrop covers all else), then locked by the timer.
    press(&mut rig, "row1");
    rig.opened(); // 2b
    let link = point(&rig, "link1");
    hover(&mut rig, link);
    for _ in 0..150 {
        frame(&mut rig);
    }
    hover(&mut rig, (790.0, 590.0));
    rig.opened(); // 3
    assert!(rig.escape(), "Escape closes the tooltip");
    rig.opened(); // 4
    assert!(rig.escape(), "Escape closes the reader");
    rig.opened(); // 5
    assert!(rig.controls.keys_blocked(), "the menu's block is still on");
    open_reader(&mut rig);
    rig.opened(); // 6
    press(&mut rig, "reader_close");
    rig.opened(); // 7
    assert!(
        rig.controls.keys_blocked(),
        "closing the reader leaves the keys blocked"
    );
    assert!(rig.escape(), "Escape closes the menu");
    rig.opened(); // 8
    assert!(
        sent(&rig).iter().all(|control| *control == Control::Pause),
        "the reader sends the simulation nothing but the menu's pause"
    );
}

#[test]
fn a_resize_centres_the_reader_again_and_closing_the_menu_closes_it() {
    let mut rig = reader_rig(
        "",
        &[
            "expect(the_reader() == nil)",
            "expect(the_reader() ~= nil)",
            r#"
local r = the_reader()
local width, height = r.window:size()
local x, y = r.window:position()
expect(math.abs(x + width / 2 - 500) <= P and math.abs(y + height / 2 - 350) <= P, "centred again in the larger view")
"#,
            "menu.close()",
            "expect(the_reader() == nil and not menu.is_open(), 'the menu took the reader with it')",
        ],
    );
    rig.opened();
    open_reader(&mut rig);
    rig.opened();
    rig.deliver_fixture_frame_at(1000, 700, 2);
    rig.opened();
    rig.opened();
    rig.opened();
}

// ---- The list and the right column, with the shipped glossary ----

#[test]
fn the_list_holds_all_92_titles_in_order_with_no_tooltip_and_the_right_column_starts_empty() {
    let mut rig = reader_rig(
        "",
        &[
            "expect(the_reader() == nil)",
            r#"
local r = the_reader()
local expected = {}
for _, key in ipairs(glossary.keys()) do
  local entry = glossary.entry(key)
  expected[#expected + 1] = {key = key, sort = string.lower(entry.title), title = entry.title}
end
table.sort(expected, function(a, b) if a.sort ~= b.sort then return a.sort < b.sort end return a.key < b.key end)
eq(#expected, 92, "the shipped glossary has 92 entries")
eq(#r.list.items, 92, "the list holds all of them")
local widest = 0
for index, item in ipairs(r.list.items) do
  eq(item.cells[1].text, expected[index].title, "row " .. index .. " is in order of the lower-case title")
  widest = math.max(widest, font.layout(item.cells[1].text).width)
end
eq(r.list.rows, 21)
eq(r.list.width, (widest + 12) * P + 5 * P, "as wide as the widest title plus 12, and the bar's room")
expect(not r.list.tooltips, "the titles have no tooltip")
expect(r.page == nil and r.chosen == nil and r.list:selected() == nil, "the right column is empty and nothing is chosen")
eq(r.list:scroll_position(), 0, "the list is at the top")
local _, _, _, row_height = hit_rect(r.list.row_keys[1])
eq(row_height, 13 * P, "13-pixel rows")
local _, _, divider_width, divider_height = quad_at(scene.quads[r.divider])
eq(divider_width, P, "a 1-pixel line") eq(divider_height, r.window.content_height, "the height of the content")
eq(scene.quads[r.divider].tint, theme_lib.kind("window").outline, "in the kind's outline")
eq(scene.quads[r.divider].x, r.list.width, "at the list column's right edge")
eq(count_hits("glossary_link_"), 0, "no link before an entry is chosen")
for key in pairs(scene.hits) do
  expect(key:match("^[a-z0-9_]+$") and #key <= 48, "a valid hit key: " .. key)
end
mark_reader()
"#,
            r#"
eq(count_hits("tooltip_panel_"), 0, "hovering a title opens no tooltip")
"#,
        ],
    );
    rig.opened();
    open_reader(&mut rig);
    rig.opened();
    let row = point(&rig, "row2");
    hover(&mut rig, row);
    for _ in 0..150 {
        frame(&mut rig);
    }
    rig.opened();
}

#[test]
fn choosing_an_entry_shows_its_gold_heading_and_its_text_wrapped_with_the_terms_in_the_term_colour()
{
    let mut rig = reader_rig(
        "",
        &[
            "expect(the_reader() == nil)",
            r#"
local r = the_reader()
r.list:scroll_to(r.index_of.location)
local x, y, w, h = hit_rect(r.list.row_keys[r.index_of.location])
mark("target", x + w / 2, y + h / 2)
"#,
            r#"
local r = the_reader()
eq(r.chosen, "location", "the click chose the entry")
eq(r.list:selected(), r.index_of.location, "its row is chosen")
eq(scene.quads[r.list.hover_quad].x, ui.HIDDEN, "the pointer is on the chosen row, which shows no hover fill over its hot one")
local found = page_text()
eq(found[1].text, "location", "the heading is the entry's title")
eq(found[1].colour, theme_lib.kind("tooltip").title_text, "in the tooltip title's gold")
eq(found[1].x, 0) eq(found[1].y, 1, "from the column's top-left")
local lines, terms = {}, {}
for index = 2, #found do
  local line = found[index]
  expect(line.x + line.width <= r.room, "line " .. index .. " stays inside the column: " .. line.text)
  expect((line.y - 19) % 13 == 0 and line.y >= 19, "lines are 13 pixels apart from 18 below the heading")
  if line.colour == theme_lib.colour("term") then terms[#terms + 1] = line.text
  else eq(line.colour, theme_lib.kind("window").text, "plain words are text: " .. line.text) end
  lines[line.y] = (lines[line.y] or "") .. line.text
end
local ys = {}
for y in pairs(lines) do ys[#ys + 1] = y end
table.sort(ys)
local text_lines = {}
for _, y in ipairs(ys) do text_lines[#text_lines + 1] = lines[y] end
eq(table.concat(text_lines, " "),
  "A single place on the map, consisting of pops and buildings. It draws on the land around it in its resource radius.",
  "the entry's text, links read as their words")
eq(table.concat(terms, " "), "pops buildings resource radius", "the linked words are in the term colour")
eq(#r.page.keys, 3, "three links, one hit key each")
local selected = scene.quads[r.list.selected_quad]
eq(selected.tint, theme_lib.kind("window").hot, "the chosen row is filled hot")
eq(selected.w, r.list.row_w - 2 * P, "its fill stops 2 UI pixels short of the row's edge")
eq(selected.h, 13 * P)
for _, key in ipairs(r.page.keys) do
  expect(key:match("^glossary_link_%d+$"), "link keys come from ui.key, never from a title or an entry key: " .. key)
end
mark_reader()
"#,
            r#"
eq(count_hits("tooltip_panel_"), 1, "hovering a linked term opens its explanation")
"#,
        ],
    );
    rig.opened();
    open_reader(&mut rig);
    rig.opened();
    press(&mut rig, "target");
    rig.opened();
    let link = point(&rig, "link1");
    hover(&mut rig, link);
    for _ in 0..150 {
        frame(&mut rig);
    }
    rig.opened();
}

// ---- The markup and the links, with known entries ----

#[test]
fn links_show_their_titles_or_words_braces_are_escaped_and_long_text_wraps_at_the_column() {
    let mut rig = reader_rig(
        "use_stub()",
        &[
            "expect(the_reader() == nil)",
            "mark_reader()",
            r#"
local r = the_reader()
eq(r.chosen, "alpha")
local found = page_text()
eq(found[1].text, "Alpha")
local joined, terms = {}, {}
for index = 2, #found do
  joined[#joined + 1] = found[index].text
  if found[index].colour == theme_lib.colour("term") then terms[#terms + 1] = found[index].text end
end
local body = table.concat(joined, "\n")
expect(body:find("Plain {brace} and a", 1, true), "escaped braces are literal braces: " .. body)
eq(table.concat(terms, ","), "linked words,Gamma", "{key|words} shows the words, {key} the linked entry's title")
expect(not body:find("beta", 1, true) and not body:find("|", 1, true) and not body:find("{gamma", 1, true), "no markup is left")
eq(#r.page.keys, 2)
"#,
            r#"
local r = the_reader()
eq(r.chosen, "delta")
local found = page_text()
eq(found[1].text, "Delta")
expect(#found > 3, "sixty words wrap to several lines: " .. #found)
for index = 2, #found do
  expect(found[index].x + found[index].width <= r.room, "inside the column")
  eq(found[index].y, 19 + 13 * (index - 2), "13-pixel lines from 18 below the heading")
  expect(not found[index].text:find("%s$"), "no space ends a line")
end
"#,
        ],
    );
    rig.opened();
    open_reader(&mut rig);
    rig.opened();
    // Alpha is the first row, Delta the third.
    press(&mut rig, "row1");
    rig.opened();
    press(&mut rig, "row3");
    rig.opened();
}

#[test]
fn clicking_a_linked_term_opens_its_entry_here_scrolls_the_list_to_its_row_and_lights_it() {
    let mut rig = reader_rig(
        "use_stub()",
        &[
            "expect(the_reader() == nil)",
            "mark_reader()",
            r#"
local r = the_reader()
eq(r.chosen, "alpha")
eq(r.list:scroll_position(), 0)
mark_reader()
"#,
            r#"
local r = the_reader()
eq(r.chosen, "gamma", "the click on the linked term chose its entry")
eq(r.index_of.gamma, 34, "gamma is the last of the 34 entries")
eq(r.list:selected(), 34, "its row is lit")
eq(r.list:scroll_position(), (34 - 21) * 13 * P, "the list scrolled the least that shows the row")
eq(scene.quads[r.list.selected_quad].y, 33 * 13 * P, "the fill is on that row")
local found = page_text()
eq(found[1].text, "Gamma", "the right column shows it")
eq(found[2].text, "Gamma text.")
expect(r.list.row_keys[34] ~= nil, "the row is built, in view")
r.list:scroll_to(r.index_of.beta)
local x, y, w, h = hit_rect(r.list.row_keys[r.index_of.beta])
mark("target", x + w / 2, y + h / 2)
"#,
            "mark_reader()",
            r#"
local r = the_reader()
eq(r.chosen, "alpha", "a link in the next entry works too")
eq(r.list:selected(), 1)
eq(r.list:scroll_position(), 0, "and the list scrolled back to the top")
eq(page_text()[1].text, "Alpha")
"#,
        ],
    );
    rig.opened();
    open_reader(&mut rig);
    rig.opened();
    press(&mut rig, "row1");
    rig.opened();
    // The second link of Alpha is {gamma}.
    press(&mut rig, "link2");
    rig.opened();
    // Beta, which names Alpha, is chosen from the list; then its link is clicked.
    press(&mut rig, "target");
    rig.opened();
    press(&mut rig, "link1");
    rig.opened();
}

// ---- Reopening ----

#[test]
fn reopening_shows_an_empty_reader_with_the_list_at_the_top() {
    let mut rig = reader_rig(
        "use_stub()",
        &[
            "expect(the_reader() == nil)",
            "mark_reader()",
            r#"
local r = the_reader()
eq(r.chosen, "alpha")
mark_reader()
"#,
            r#"
local r = the_reader()
eq(r.chosen, "gamma")
expect(r.list:scroll_position() > 0, "the list is scrolled")
reader_lib.close()
expect(the_reader() == nil, "closed")
menu_button("glossary").on_click()
local again = the_reader()
expect(again ~= nil and again ~= r, "a new reader")
expect(again.page == nil and again.chosen == nil and again.list:selected() == nil, "empty, nothing chosen")
eq(again.list:scroll_position(), 0, "the list at the top")
eq(count_hits("glossary_link_"), 0, "no link of the old page is left")
"#,
        ],
    );
    rig.opened();
    open_reader(&mut rig);
    rig.opened();
    press(&mut rig, "row1");
    rig.opened();
    press(&mut rig, "link2");
    rig.opened();
}
