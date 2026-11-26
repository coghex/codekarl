//! The character screen's pop tab and its life log window (CK-378): the shipped `scripts/ui/main.lua` in a real UI VM over a stub `game` whose
//! `game.character_pop` serves the exact pop of a character at a revision, `game.place` the economy views, and `game.character_log` the dated lines
//! of a life, with what the tab and the window draw recorded as they draw it.
//!
//! A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use super::character_relations::RELATIONS;
use super::characters::{
    CHARACTERS, card_point, click, note_line, open_characters, screen_tab_point,
};
use super::*;

/// What the pop tests add to the characters' and relations' preludes: two noble pops at `ailech` (slot 0 and slot 1) and a second place, their
/// economy views at three revisions, `game.character_pop` over them, a life log of thirty long lines for Niall and none for his wife, and observers.
const POP: &str = r#"
local function pop_row(kind, workers, dependents, food_days, nec, nic, lux, sat, eq, births, deaths)
  return {kind = kind, culture = "ulaid", religion = "christianity", workers = workers, dependents = dependents, employed = 0,
    unemployed = workers, food_days = food_days, food = {{good = "oats", units = workers, unit = "bushel", share_pm = 1000}},
    necessities = nec, niceties = nic, luxuries = lux, satisfaction = sat, equilibrium = eq, births = births, deaths = deaths,
    basic_housing = 0, makeshift_housing = 0, crowding_pm = false}
end
local function ailech(revision)
  local base = {}
  for key, value in pairs(VIEWS[1].rowan) do base[key] = value end
  base.id = "ailech"
  if revision == 1 then
    base.pops = {pop_row("noble", 34, 58, 214, 1000, 700, 300, 62, 58, 1, 0), pop_row("noble", 12, 20, 40, 810, 455, 8, -12, 5, 2, 3)}
  else
    base.pops = {pop_row("noble", 30, 50, 200, 900, 650, 250, 40, 45, 0, 0), pop_row("noble", 99, 77, 11, 500, 400, 10, 10, 20, 9, 8)}
  end
  return base
end
VIEWS[1].ailech, VIEWS[2].ailech, VIEWS[3] = ailech(1), ailech(2), {ailech = ailech(2)}
place_data[#place_data + 1] = {id = "ailech", name = "Ailech", kind = "town", size = 3}
-- Who is in which pop (by slot): at revision 1 Niall, Dunlaith and Aed are in the second pop; at revision 2 Dunlaith has moved to the first.
local SLOT = {[1] = {[1] = 1, [2] = 1, [3] = 0, [4] = 1, [5] = 0}, [2] = {[1] = 1, [2] = 0, [3] = 0, [5] = 0}}
SLOT[3] = SLOT[1]
pop_calls = {}
-- A crowded pop (`BIG_POP = true`): fifteen others more important than Niall, so that he ranks sixteenth of sixteen.
local function crowd()
  local list = {}
  for n = 1, 15 do list[n] = card(100 + n, "Crowd " .. n, "m", 30, "", "peasant", {}, n - 1) end
  list[16] = card(1, "Niall mac Fergaile", "m", 50, "", "ruler", {ruler = true}, 15, "Frossach")
  return list
end
game.character_pop = function(id, revision)
  pop_calls[#pop_calls + 1] = id .. "@" .. revision
  if BIG_POP == true and SLOT[revision] ~= nil and id == 1 then return {place = "ailech", slot = 1, characters = crowd()} end
  local slots = SLOT[revision]
  if slots == nil then
    if revision < 9 then error("game.character_pop: revision " .. revision .. " is missing") end
    error("game.character_pop: revision " .. revision .. " is retired")
  end
  if slots[id] == nil then error("game.character_pop: no character " .. id .. " at revision " .. revision) end
  local list = {}
  for _, c in ipairs(CH[revision]) do
    if slots[c.id] == slots[id] then list[#list + 1] = c end
  end
  return {place = "ailech", slot = slots[id], characters = list}
end

-- The life logs: Niall's thirty long lines (a new one at revision 2), Dunlaith's none.
local function line(n, text)
  return {day = 1 + n % 28, month = 1 + n % 12, year = 741 + n // 2, text = text}
end
local LINES = {}
for n = 1, 30 do LINES[n] = line(n, string.format("entry %d: %s", n, string.rep("and so it went ", 12))) end
local LATER_LINES = {}
for n = 1, 30 do LATER_LINES[n] = LINES[n] end
LATER_LINES[31] = line(31, "entry 31: was named heir")
LOG = {[1] = {[1] = LINES, [2] = {}}, [2] = {[1] = LATER_LINES, [2] = {}}, [3] = {[1] = LINES, [2] = {}}}
log_calls = {}
game.character_log = function(id, revision)
  log_calls[#log_calls + 1] = id .. "@" .. revision
  local at = LOG[revision]
  if at == nil then
    if revision < 9 then error("game.character_log: revision " .. revision .. " is missing") end
    error("game.character_log: revision " .. revision .. " is retired")
  end
  if at[id] == nil then error("game.character_log: no character " .. id .. " at revision " .. revision) end
  return at[id]
end

-- The pop tab: what was read and the order of the characters row.
function announce_pop(tag)
  local data = screen_object.pop_data
  if data == nil then
    note("pop " .. tag .. " none")
    return
  end
  local ids = {}
  for index, c in ipairs(data.characters) do ids[index] = c.id end
  note(string.format("pop %s place=%s slot=%d revision=%d workers=%d dependents=%d food_days=%d members=%s", tag, data.place, data.slot,
    data.revision, data.pop.workers, data.pop.dependents, data.pop.food_days, table.concat(ids, ",")))
end
-- The frames drawn since the last call to `forget_frames`, by their line colour.
drawn_frames = {}
do
  local draw_library = require("lib.draw")
  local real_frame = draw_library.frame
  draw_library.frame = function(result, group, x, y, width, height, fill, line)
    drawn_frames[#drawn_frames + 1] = {x = x, y = y, width = width, height = height, line = line}
    return real_frame(result, group, x, y, width, height, fill, line)
  end
end
function announce_current_frames()
  local theme_library = require("lib.theme")
  local count = 0
  for _, frame in ipairs(drawn_frames) do
    if frame.line == theme_library.kind("social").hot and frame.width == 26 then count = count + 1 end
  end
  note("pop_frames " .. count)
end
-- The fill widths of the needs bars of the location panel's pops tab for `ailech` (its ten-pixel tracks, three to a card, in card order).
function announce_location_needs()
  local theme_library = require("lib.theme")
  local ui_library = require("lib.ui")
  local quads = {}
  local real = render.create_quad
  render.create_quad = function(spec)
    quads[#quads + 1] = spec
    return real(spec)
  end
  select_place("ailech", "Ailech")
  tabs_by_title.Ailech:select("pops", true)
  render.create_quad = real
  local fills = {}
  for _, track in ipairs(quads) do
    if track.width == 10 * ui_library.P and track.height == 3 * ui_library.P and track.tint == theme_library.colour("track") then
      local width = 0
      for _, quad in ipairs(quads) do
        if quad.x == track.x and quad.y == track.y and quad.height == track.height and quad.tint == theme_library.colour("text") then
          width = quad.width // ui_library.P
        end
      end
      fills[#fills + 1] = width
    end
  end
  note("loc_needs " .. table.concat(fills, ","))
end
function announce_pop_items()
  local heights = {}
  for index, item in ipairs(screen_stack.items) do heights[index] = item.height end
  note("pop_items " .. #screen_stack.items .. " " .. table.concat(heights, ","))
end
-- The tooltip of every hit of the item `index`: its part and words.
function announce_pop_tips(index)
  for _, hit in ipairs(screen_stack.items[index].hits) do
    local x, y = middle_of(index, hit)
    local content = asked(index, x, y)
    note(string.format("pop_tip %d %s %g %g %s | %s", index, hit.part, x, y, tostring(hit.open ~= nil),
      content == nil and "<none>" or flat(content)))
  end
end
-- The satisfaction row: the track, the fill and the tick, as drawn.
function announce_pop_bar()
  local theme_library = require("lib.theme")
  local track, fill, tick
  for _, rect in ipairs(drawn_rects) do
    if rect.height == 3 and rect.colour == theme_library.colour("track") and (track == nil or rect.width > track.width) then track = rect end
  end
  for _, rect in ipairs(drawn_rects) do
    if rect.height == 3 and track ~= nil and rect.y == track.y and rect.x == track.x and rect.colour == theme_library.colour("text") then fill = rect end
    if rect.height == 7 and rect.width == 1 and rect.colour == theme_library.colour("dim") and track ~= nil and rect.y == track.y - 2 then tick = rect end
  end
  note(string.format("pop_bar track=%d fill=%d tick=%d", track and track.width or -1, fill and fill.width or 0, tick and (tick.x - track.x) or -1))
end
-- The needs rows' fills against their tracks: the fill width and the track width of each row below the satisfaction bar.
function announce_pop_needs()
  local theme_library = require("lib.theme")
  local rows = {}
  for _, rect in ipairs(drawn_rects) do
    if rect.height == 3 and rect.colour == theme_library.colour("track") then rows[#rows + 1] = rect end
  end
  table.sort(rows, function(a, b) return a.y < b.y end)
  local out = {}
  for _, track in ipairs(rows) do
    local width = 0
    for _, rect in ipairs(drawn_rects) do
      if rect.height == 3 and rect.y == track.y and rect.x == track.x and rect.colour == theme_library.colour("text") then width = rect.width end
    end
    out[#out + 1] = track.width .. ":" .. width
  end
  note("pop_needs " .. table.concat(out, " "))
end
function announce_log_button()
  local button = screen_object.log_button
  if button == nil then
    note("log_button none")
    return
  end
  local width, height = button:size()
  local ox, oy = screen_object.window:content_origin()
  note(string.format("log_button %g %g tab=%s", ox + button.x + width / 2, oy + button.y + height / 2, screen_object:current_tab()))
end
-- The life log window: whether it is open, its title, the lines it holds, where it stands and which entries are drawn.
function announce_log()
  local log = screen_object.log
  if not log:is_open() then
    note("log closed screen=" .. tostring(screen_object:is_open()))
    return
  end
  local theme_library = require("lib.theme")
  local stack = log.stack
  -- The lines the stack holds views of are the ones inside it: the first and last of them.
  local first, last, dated, empty = nil, nil, 0, false
  for index in pairs(stack.views) do
    first = math.min(first or index, index)
    last = math.max(last or index, index)
  end
  for _, entry in ipairs(drawn_texts) do
    if entry.text:match("^%d+ %a%a%a %d+$") and entry.colour == theme_library.colour("dim") then dated = dated + 1 end
    if entry.text == "nothing yet" and entry.colour == theme_library.colour("dim") then empty = true end
  end
  note(string.format("log open title=%s lines=%d items=%d at_end=%s scroll=%g drawn=%s-%s dated=%d empty=%s", log.window.title, #log.lines,
    #stack.items, tostring(stack:at_end()), stack:scroll_position(), tostring(first), tostring(last), dated, tostring(empty)))
end
-- The date texts drawn, in order.
function announce_log_dates()
  local dates = {}
  for _, entry in ipairs(drawn_texts) do
    if entry.text:match("^%d+ %a%a%a %d+$") then dates[#dates + 1] = entry.text end
  end
  note("log_dates " .. table.concat(dates, ","))
end
function announce_shown_place()
  local shown = window_objects["Ailech"]
  note("shown_place " .. tostring(shown ~= nil and not shown.closed and "Ailech" or "none") .. " screen=" .. tostring(screen_object:is_open()))
end
"#;

fn pop_rig(steps: &[&str]) -> Rig {
    let prelude = format!("{CHARACTERS}{RELATIONS}{POP}");
    rig_with_prelude_at(&prelude, steps, 1800, 900, 2)
}

const OPEN: [&str; 3] = [
    "ctime(1, 1, 0) announce_bars()",
    "announce_cards() announce_ledger_origin()",
    "announce_screen() announce_screen_tabs() forget_drawn()",
];

fn steps_after_open<'a>(after: &[&'a str]) -> Vec<&'a str> {
    OPEN.iter().chain(after.iter()).copied().collect()
}

/// Opens the screen of the `n`th card of the ledger and its pop tab: after this the next step runs over the tab.
fn open_pop(rig: &mut Rig, card: usize) {
    open_characters(rig);
    let (x, y) = card_point(rig, card);
    click(rig, x, y);
    rig.opened();
    let (x, y) = screen_tab_point(rig, "pop");
    click(rig, x, y);
    rig.opened();
}

fn notes_with(rig: &Rig, prefix: &str) -> Vec<String> {
    notes(rig)
        .into_iter()
        .filter(|line| line.starts_with(prefix))
        .collect()
}

/// The point of the note `prefix` ("name x y ...").
fn point_of(rig: &Rig, prefix: &str) -> (f64, f64) {
    let line = note_line(rig, prefix);
    let mut values = line[prefix.len()..].split_whitespace();
    (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    )
}

/// The `pop_tip` notes of the item `index`: the part, the point to press, whether a press does something and the words.
fn tips(rig: &Rig, index: usize) -> Vec<(String, f64, f64, bool, String)> {
    let prefix = format!("pop_tip {index} ");
    notes(rig)
        .into_iter()
        .filter(|line| line.starts_with(&prefix))
        .map(|line| {
            let (head, words) = line.split_once(" | ").expect("a tooltip");
            let mut values = head[prefix.len()..].split_whitespace();
            let part = values.next().unwrap().to_owned();
            let mut number = || -> f64 { values.next().unwrap().parse().unwrap() };
            let (x, y) = (number(), number());
            let live = values.next().unwrap() == "true";
            (part, x, y, live, words.to_owned())
        })
        .collect()
}

#[test]
fn the_tab_shows_the_identity_the_people_the_satisfaction_the_needs_and_the_characters_of_the_exact_pop()
 {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_screen() announce_pop('niall') announce_pop_items() announce_pop_bar() announce_pop_needs() \
         note('pop_texts ' .. table.concat({tostring(text_drawn('noble')), tostring(text_drawn('at')), tostring(text_drawn('Ailech')), \
         tostring(text_drawn('open location')), tostring(text_drawn('12')), tostring(text_drawn('workers')), tostring(text_drawn('dependents')), \
         tostring(text_drawn('food days')), tostring(text_drawn('-12/+5')), tostring(text_drawn('81%')), \
         tostring(text_drawn('45%')), tostring(text_drawn('0%')), tostring(text_drawn('politics')), tostring(text_drawn('wealth')), \
         tostring(text_drawn('people')), tostring(text_drawn('satisfaction')), tostring(text_drawn('characters')), \
         tostring(text_drawn('necessities')), tostring(text_drawn('niceties')), tostring(text_drawn('luxuries'))}, ' '))",
    ]));
    open_pop(&mut rig, 1);
    // Niall is in the second pop of Ailech at revision 1: its numbers (12 workers, 20 dependents, 40 food days) and its members, most important first.
    let pop = note_line(&rig, "pop niall ");
    assert_eq!(
        pop,
        "pop niall place=ailech slot=1 revision=1 workers=12 dependents=20 food_days=40 members=1,2,4"
    );
    assert!(
        note_line(&rig, "screen id=1").contains("tab=pop"),
        "{:?}",
        notes_with(&rig, "screen")
    );
    // Four sections: the identity, the people, the satisfaction with the needs, and the characters.
    assert!(note_line(&rig, "pop_items ").starts_with("pop_items 4 "));
    // The words: the kind at the place, the button, the numbers with their words and the three needs; politics and wealth are left out.
    let texts = note_line(&rig, "pop_texts ");
    let seen: Vec<&str> = texts["pop_texts ".len()..].split(' ').collect();
    // noble at Ailech, open location, 12 workers dependents food days, -12/+5, 81% 45% 0%, no politics, no wealth, three headings, the three needs.
    assert_eq!(
        seen,
        [
            "true", "true", "true", "true", "true", "true", "true", "true", "true", "true", "true",
            "true", "false", "false", "true", "true", "true", "true", "true", "true"
        ],
        "{texts}"
    );
    // The bar is on -100..+100: -12 fills 44% of it and the tick of the equilibrium +5 stands at 52.5%, as one line of pixels.
    let bar = note_line(&rig, "pop_bar ");
    let value = |key: &str| -> i64 {
        bar.split(key)
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (track, fill, tick) = (value("track="), value("fill="), value("tick="));
    assert!(track > 20, "{bar}");
    assert_eq!(
        fill,
        (track * 88 * 2 + 200) / 400,
        "{bar}: -12 is 88 of 200 from the left end"
    );
    assert_eq!(
        tick,
        (track * 105 * 2 + 200) / 400,
        "{bar}: +5 is 105 of 200 from the left end"
    );
    // The needs: 810, 455 and 8 per mille, filled by floor division of each track as the location panel fills its own.
    let needs = note_line(&rig, "pop_needs ");
    let rows: Vec<(i64, i64)> = needs["pop_needs ".len()..]
        .split(' ')
        .skip(1)
        .map(|row| {
            let (track, fill) = row.split_once(':').unwrap();
            (track.parse().unwrap(), fill.parse().unwrap())
        })
        .collect();
    assert_eq!(rows.len(), 3, "{needs}");
    for ((track, fill), per_mille) in rows.iter().zip([810_i64, 455, 8]) {
        assert_eq!(
            *fill,
            track * per_mille / 1000,
            "{needs}: the location panel's rule, floor division"
        );
    }
}

#[test]
fn the_satisfaction_bar_shows_a_positive_value_and_the_three_needs_read_as_the_location_panels_pops_tab_does()
 {
    // Fothad (card 3) is in the first pop at revision 1: +62 against an equilibrium of +58, and 100%, 70% and 30% of the needs.
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_pop('fothad') announce_pop_bar() announce_pop_needs() \
         note('pop_texts ' .. table.concat({tostring(text_drawn('+62/+58')), tostring(text_drawn('100%')), \
         tostring(text_drawn('70%')), tostring(text_drawn('30%'))}, ' ')) announce_pop_tips(3)",
    ]));
    open_pop(&mut rig, 3);
    assert_eq!(
        note_line(&rig, "pop fothad "),
        "pop fothad place=ailech slot=0 revision=1 workers=34 dependents=58 food_days=214 members=3,5"
    );
    assert_eq!(
        note_line(&rig, "pop_texts "),
        "pop_texts true true true true"
    );
    let bar = note_line(&rig, "pop_bar ");
    let value = |key: &str| -> i64 {
        bar.split(key)
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (track, fill, tick) = (value("track="), value("fill="), value("tick="));
    assert_eq!(fill, (track * 162 * 2 + 200) / 400, "{bar}");
    assert_eq!(tick, (track * 158 * 2 + 200) / 400, "{bar}");
    assert!(tick < fill, "the pop is above its equilibrium: {bar}");
    // The tooltips of the satisfaction row and of the three needs are the glossary's, the ones the location panel's pops tab gives.
    let tips = tips(&rig, 3);
    let parts: Vec<&str> = tips.iter().map(|tip| tip.0.as_str()).collect();
    assert_eq!(
        parts,
        ["satisfaction", "necessities", "niceties", "luxuries"]
    );
}

#[test]
fn every_number_has_the_location_panels_tooltip_for_the_exact_slot_and_the_revision_of_the_tab() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_pop_tips(2) announce_pop_tips(1)",
        // The newest revision the panel knows moves on to 99, which the character reads refuse: the tab keeps revision 1 and its tooltips too.
        "CH[99] = nil ctime(3, 99, 0) announce_pop('kept') announce_pop_tips(2)",
    ]));
    open_pop(&mut rig, 1);
    let people = tips(&rig, 2);
    let words: Vec<(&str, &str)> = people
        .iter()
        .map(|tip| (tip.0.as_str(), tip.4.as_str()))
        .collect();
    // The second pop of Ailech at revision 1: workers 12, dependents 20, births 2 and deaths 3; its food from the oats it holds.
    assert_eq!(
        words,
        [
            ("workers", "workers 12 / dependents 20 / births 2, deaths 3"),
            (
                "dependents",
                "workers 12 / dependents 20 / births 2, deaths 3"
            ),
            ("food_days", "oats 12 bushels, 100% of meals"),
        ]
    );
    let identity = tips(&rig, 1);
    let parts: Vec<&str> = identity.iter().map(|tip| tip.0.as_str()).collect();
    assert_eq!(parts, ["kind", "culture", "religion", "open_location"]);
    // A newer revision the panel pins does not change the tab's.
    let (x, y) = screen_tab_point(&rig, "pop");
    click(&mut rig, x, y);
    rig.opened();
    assert!(
        note_line(&rig, "pop kept ").contains("revision=1 workers=12"),
        "{:?}",
        notes_with(&rig, "pop ")
    );
    let again: Vec<String> = tips(&rig, 2)
        .iter()
        .skip(3)
        .map(|tip| tip.4.clone())
        .collect();
    assert_eq!(
        again,
        [
            "workers 12 / dependents 20 / births 2, deaths 3",
            "workers 12 / dependents 20 / births 2, deaths 3",
            "oats 12 bushels, 100% of meals"
        ],
        "the tab's revision, not the pinned 99"
    );
}

#[test]
fn two_pops_of_one_kind_show_their_own_numbers_and_a_character_who_moves_shows_the_new_one() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_pop('before') ctime(5, 2, 0)",
        "announce_pop('after') announce_pop_tips(2)",
    ]));
    // Dunlaith (card 2) is in the second pop at revision 1, with Niall and Aed; at revision 2 she is in the first, with Fothad and Gormlaith.
    open_pop(&mut rig, 2);
    assert_eq!(
        note_line(&rig, "pop before "),
        "pop before place=ailech slot=1 revision=1 workers=12 dependents=20 food_days=40 members=1,2,4"
    );
    // The revision-2 view arrives and the screen follows it.
    rig.opened();
    assert_eq!(
        note_line(&rig, "pop after "),
        "pop after place=ailech slot=0 revision=2 workers=30 dependents=50 food_days=200 members=2,3,5"
    );
    let first = tips(&rig, 2).into_iter().next().unwrap();
    assert_eq!(first.4, "workers 30 / dependents 50 / births 0, deaths 0");
}

#[test]
fn a_click_on_another_portrait_opens_its_screen_and_the_viewed_one_is_framed_and_does_nothing() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_rel_hits(4)",
        "announce_screen()",
    ]));
    open_pop(&mut rig, 1);
    // Niall's pop at revision 1: Niall, Dunlaith and Aed, in that order; only the others open a screen.
    let hits: Vec<String> = notes_with(&rig, "rel_hit 4 ");
    assert_eq!(hits.len(), 3, "{hits:?}");
    let flags: Vec<bool> = hits
        .iter()
        .map(|line| line.split(" | ").next().unwrap().ends_with("true"))
        .collect();
    assert_eq!(flags, [false, true, true]);
    assert!(
        hits[0].contains("Niall mac Fergaile Frossach / "),
        "{}",
        hits[0]
    );
    assert!(hits[1].contains("Dunlaith / "), "{}", hits[1]);
    // Pressing the second portrait replaces the screen with Dunlaith's, on the first tab.
    let second = &hits[1];
    let mut values = second["rel_hit 4 2 ".len()..].split_whitespace();
    let (x, y): (f64, f64) = (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    );
    click(&mut rig, x, y);
    rig.opened();
    let screen = note_line(&rig, "screen id=");
    assert!(screen.contains("id=2 tab=character"), "{screen}");
}

#[test]
fn open_location_closes_the_screen_and_shows_the_place_in_the_location_panel() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_pop_tips(1) announce_shown_place()",
        "announce_shown_place()",
    ]));
    open_pop(&mut rig, 1);
    let button = tips(&rig, 1)
        .into_iter()
        .find(|tip| tip.0 == "open_location")
        .expect("the button");
    assert!(button.3, "a press on it does something");
    assert_eq!(button.4, "open Ailech");
    click(&mut rig, button.1, button.2);
    rig.opened();
    assert_eq!(
        note_line(&rig, "shown_place "),
        "shown_place Ailech screen=false"
    );
}

#[test]
fn the_life_log_button_stands_on_every_tab() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_log_button()",
        "announce_log_button()",
        "announce_log_button()",
        "announce_log_button()",
    ]));
    open_pop(&mut rig, 1);
    // The pop tab is showing (the rig opened it); the button is there, and on each of the other tabs.
    assert!(
        note_line(&rig, "log_button ").ends_with("tab=pop"),
        "{:?}",
        notes_with(&rig, "log_button")
    );
    for key in ["actions", "relations", "character"] {
        let (x, y) = screen_tab_point(&rig, key);
        click(&mut rig, x, y);
        rig.opened();
        assert!(
            note_line(&rig, "log_button ").ends_with(&format!("tab={key}")),
            "{key}: {:?}",
            notes_with(&rig, "log_button")
        );
    }
    assert!(
        !notes_with(&rig, "log_button none").is_empty()
            || notes_with(&rig, "log_button ").len() == 4,
        "{:?}",
        notes_with(&rig, "log_button")
    );
}

/// Opens Niall's screen and presses its life-log button: after this the next step runs over the open window.
fn open_log(rig: &mut Rig, card: usize) {
    open_characters(rig);
    let (x, y) = card_point(rig, card);
    click(rig, x, y);
    rig.opened();
    rig.opened();
    let (x, y) = point_of(rig, "log_button ");
    click(rig, x, y);
    rig.opened();
}

#[test]
fn the_window_lists_the_lines_oldest_first_with_dates_opens_at_the_newest_and_scrolls() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_log_button() forget_drawn()",
        "announce_log() forget_drawn() screen_object.log.stack:set_pos(0)",
        "announce_log() announce_log_dates() forget_drawn() screen_object.log.stack:scroll_to_end()",
        "announce_log()",
    ]));
    open_log(&mut rig, 1);
    // Opened: titled by the name, all thirty lines in the stack, at the end with the newest held and the oldest not, each date drawn dim.
    let opened = note_line(&rig, "log open ");
    assert!(
        opened.contains("title=life of Niall mac Fergaile lines=30 items=30 at_end=true"),
        "{opened}"
    );
    assert!(
        opened.contains("drawn=") && opened.contains("-30"),
        "{opened}"
    );
    assert!(
        !opened.contains("scroll=0 ") && !opened.contains("drawn=1-"),
        "{opened}"
    );
    assert!(!opened.contains("dated=0"), "{opened}");
    // Scrolled to the top: the oldest line is drawn, in its place, and the newest is not.
    rig.opened();
    let top = notes_with(&rig, "log open ").pop().unwrap();
    assert!(top.contains("at_end=false scroll=0 drawn=1-"), "{top}");
    assert!(!top.contains("-30 "), "{top}");
    let dates = note_line(&rig, "log_dates ");
    // The first line is dated 2 Feb 741 and the next 3 Mar 741: oldest first.
    assert!(
        dates.starts_with("log_dates 2 Feb 741,3 Mar 742,"),
        "{dates}"
    );
    // And back to the end.
    rig.opened();
    let end = notes_with(&rig, "log open ").pop().unwrap();
    assert!(end.contains("at_end=true") && end.contains("-30"), "{end}");
}

#[test]
fn an_empty_log_says_nothing_yet_in_the_dim_colour() {
    // Dunlaith's log has no lines.
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_log_button() forget_drawn()",
        "announce_log()",
    ]));
    open_log(&mut rig, 2);
    let opened = note_line(&rig, "log open ");
    assert!(
        opened.contains("lines=0 items=1 at_end=true") && opened.contains("empty=true"),
        "{opened}"
    );
}

#[test]
fn escape_closes_the_log_first_then_the_screen() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_log_button()",
        "announce_log()",
        "announce_log() announce_screen()",
        "announce_log() announce_screen()",
    ]));
    open_log(&mut rig, 1);
    assert!(note_line(&rig, "log open ").contains("title=life of"));
    // One Escape closes the log and leaves the screen.
    assert!(rig.escape(), "Escape closes the topmost window");
    rig.opened();
    assert_eq!(note_line(&rig, "log closed "), "log closed screen=true");
    // The next closes the screen.
    assert!(rig.escape(), "Escape closes the screen");
    rig.opened();
    assert_eq!(note_line(&rig, "log closed "), "log closed screen=false");
}

#[test]
fn a_new_revision_with_a_new_line_shows_it_at_the_end_and_keeps_the_scroll_when_not_at_the_end() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_log_button() forget_drawn()",
        "announce_log() forget_drawn() ctime(5, 2, 0) announce_log()",
        "forget_drawn() screen_object.log.stack:set_pos(0) forget_drawn() ctime(7, 3, 0) announce_log() note('log_reads ' .. table.concat(log_calls, ','))",
    ]));
    open_log(&mut rig, 1);
    // Revision 2 brought a thirty-first line while the window was at the end (the opening step); then it is scrolled to the top and revision 3 (the
    // thirty lines again) arrives: the scroll position is kept.
    rig.opened();
    let last = notes_with(&rig, "log open ").pop().unwrap();
    assert!(
        last.contains("lines=30") && last.contains("at_end=false scroll=0"),
        "{last}"
    );
    // Reads: at opening (revision 1), at each new revision, and never each frame.
    assert_eq!(
        note_line(&rig, "log_reads "),
        "log_reads 1@1,1@2,1@3",
        "read when it opens and when a new revision arrives, nothing more"
    );
    let at_new = notes_with(&rig, "log open ");
    assert!(
        at_new.iter().any(|line| line.contains("lines=31")
            && line.contains("at_end=true")
            && line.contains("-31")),
        "{at_new:?}"
    );
}

#[test]
fn a_world_change_closes_the_log_with_the_screen_and_a_refused_read_changes_nothing() {
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_log_button() forget_drawn()",
        "CH[99] = nil ctime(3, 99, 0) announce_log() announce_screen()",
        "ctime(5, 2, 1) announce_log()",
    ]));
    open_log(&mut rig, 1);
    // A revision the reads refuse changes nothing: the window and the screen stay.
    assert!(
        note_line(&rig, "log open ").contains("lines=30"),
        "{:?}",
        notes_with(&rig, "log")
    );
    // A new world closes the log with the screen.
    rig.opened();
    assert_eq!(note_line(&rig, "log closed "), "log closed screen=false");
}

/// The shipped UI over **real** data (CK-378 witness): the Lua forms of what `game.characters(1)`, `game.character(id, 1)`, `game.character_pop(id, 1)`,
/// `game.place(place, 1)` and `game.character_log(id, 1)` returned from the Rust view of the shipped 768 start. The card of importance `rank` is clicked, then the
/// pop tab, and then the life-log button; returns the notes the steps made.
pub(crate) fn real_pop_and_log(
    cards: &str,
    record: &str,
    pop: &str,
    place: &str,
    log: &str,
    id: i64,
    rank: usize,
) -> Vec<String> {
    let prelude = format!(
        "{CHARACTERS}{RELATIONS}{POP}\nREAL_CARDS = {cards}\nREAL_RECORD = {record}\nREAL_POP = {pop}\nREAL_PLACE = {place}\nREAL_LOG = {log}\nCH = {{[1] = REAL_CARDS}}\n\
         place_data[#place_data + 1] = {{id = REAL_POP.place, name = 'Ailech', kind = 'town', size = 3}}\n\
         game.characters = function(revision) return REAL_CARDS end\n\
         game.character = function(id, revision)\n\
           if id ~= {id} or revision ~= 1 then error('game.character: no character ' .. id .. ' at revision ' .. revision) end\n\
           return REAL_RECORD\n\
         end\n\
         game.character_pop = function(id, revision)\n\
           if id ~= {id} or revision ~= 1 then error('game.character_pop: no character ' .. id .. ' at revision ' .. revision) end\n\
           return REAL_POP\n\
         end\n\
         local stub_place = game.place\n\
         game.place = function(place, revision)\n\
           if place == REAL_POP.place and revision == 1 then return REAL_PLACE end\n\
           return stub_place(place, revision)\n\
         end\n\
         game.character_log = function(id, revision)\n\
           if id ~= {id} or revision ~= 1 then error('game.character_log: no character ' .. id .. ' at revision ' .. revision) end\n\
           return REAL_LOG\n\
         end\n"
    );
    let mut rig = rig_with_prelude_at(
        &prelude,
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_cards() announce_ledger_origin()",
            "announce_screen() announce_screen_tabs() forget_drawn()",
            "announce_screen() announce_pop('real') announce_pop_items() announce_pop_bar() announce_log_button() forget_drawn()",
            "announce_log()",
        ],
        1800,
        900,
        2,
    );
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, rank + 1);
    click(&mut rig, x, y);
    rig.opened();
    let (x, y) = screen_tab_point(&rig, "pop");
    click(&mut rig, x, y);
    rig.opened();
    let (x, y) = point_of(&rig, "log_button ");
    click(&mut rig, x, y);
    rig.opened();
    notes(&rig)
        .into_iter()
        .filter(|line| {
            ["pop ", "pop_", "log ", "log_", "screen"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect()
}

#[test]
fn the_needs_bars_fill_by_the_location_panels_rule_for_the_same_pop_and_revision() {
    // The second pop of Ailech at revision 1 needs 810, 455 and 8 per mille. The location panel's pops tab shows the same pop in ten-pixel tracks, filled by
    // floor division: 8, 4 and 0 pixels (nearest-pixel rounding would give 8, 5 and 0). The tab's wider tracks fill by the same rule.
    let mut rig = pop_rig(&steps_after_open(&[
        "announce_pop_needs() announce_location_needs()",
    ]));
    open_pop(&mut rig, 1);
    let location = note_line(&rig, "loc_needs ");
    let location: Vec<i64> = location["loc_needs ".len()..]
        .split(',')
        .map(|width| width.parse().unwrap())
        .collect();
    // Two pop cards, three bars each: the first pop 10, 7 and 3 pixels, the second 8, 4 and 0.
    assert_eq!(location, [10, 7, 3, 8, 4, 0], "the location panel's fills");
    let needs = note_line(&rig, "pop_needs ");
    let tab: Vec<(i64, i64)> = needs["pop_needs ".len()..]
        .split(' ')
        .skip(1)
        .map(|row| {
            let (track, fill) = row.split_once(':').unwrap();
            (track.parse().unwrap(), fill.parse().unwrap())
        })
        .collect();
    for (((track, fill), per_mille), at_location) in
        tab.iter().zip([810_i64, 455, 8]).zip(&location[3..])
    {
        assert_eq!(*fill, track * per_mille / 1000, "{needs}");
        assert_eq!(
            *at_location,
            10 * per_mille / 1000,
            "the same rule at ten pixels"
        );
    }
}

#[test]
fn a_crowded_pop_keeps_the_viewed_character_framed_in_its_place_and_the_chip_lists_the_hidden_ones()
{
    // Niall ranks sixteenth of sixteen; two rows hold fourteen places, the last the "+N" chip, so thirteen portraits show.
    let mut rig = pop_rig(&[
        "BIG_POP = true ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin()",
        "announce_screen() announce_screen_tabs() forget_drawn()",
        "announce_rel_hits(4) announce_current_frames()",
    ]);
    open_pop(&mut rig, 1);
    let hits = notes_with(&rig, "rel_hit 4 ");
    assert_eq!(hits.len(), 14, "thirteen portraits and the chip: {hits:?}");
    let tip = |line: &String| line.split(" | ").nth(1).unwrap().to_owned();
    let opens = |line: &String| line.split(" | ").next().unwrap().ends_with("true");
    // The thirteenth portrait is the viewed character, in importance order after the twelve most important, and presses nothing.
    assert!(
        tip(&hits[12]).starts_with("Niall mac Fergaile Frossach / "),
        "{}",
        tip(&hits[12])
    );
    assert!(!opens(&hits[12]));
    assert!(hits[..12].iter().all(opens));
    assert!(tip(&hits[0]).starts_with("Crowd 1 / "));
    assert!(tip(&hits[11]).starts_with("Crowd 12 / "));
    // The chip lists the actual hidden members, not the viewed one.
    assert_eq!(tip(&hits[13]), "+3 more / Crowd 13 / Crowd 14 / Crowd 15");
    assert_eq!(
        note_line(&rig, "pop_frames "),
        "pop_frames 1",
        "one frame ringing the viewed portrait"
    );
}
