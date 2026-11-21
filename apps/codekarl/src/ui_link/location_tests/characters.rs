//! The ledger's Characters tab and the character screen (CK-375): the shipped `scripts/ui/main.lua` in a real UI VM over a stub `game` whose
//! `game.characters` and `game.character` serve fixture revisions, with what the cards and the screen draw recorded as they draw it.
//!
//! A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use super::*;

/// The Lua the tests of this file share, after the location tests' own prelude and before `main.lua`: the fixture characters of three
/// revisions, `game.characters` and `game.character` over them, and observers of what is drawn.
const CHARACTERS: &str = r##"
local BODY = {"height", "weight", "strength", "wit", "intellect", "cunning", "longevity", "constitution", "fecundity"}
local HUMOURS = {"blood", "yellow_bile", "black_bile", "phlegm"}
local SKILLS = {"leadership", "craftsmanship", "seamanship", "scholarship", "horsemanship", "martial_prowess", "oratory",
  "administration", "theology", "medicine", "husbandry"}
local VIRTUE = {"pride_humility", "greed_charity", "lust_chastity", "envy_kindness", "gluttony_temperance", "wrath_patience",
  "sloth_diligence"}
local MANNER = {"guarded_open", "deceitful_honest", "suspicious_trusting", "wary_daring", "traditional_curious", "yielding_stubborn",
  "solitary_gregarious", "content_ambitious", "harsh_merciful"}
local function keyed(keys, values)
  local out = {}
  for index, key in ipairs(keys) do out[index] = {key = key, value = values[index]} end
  return out
end
local function card(id, name, sex, age, house, role, flags, importance, epithet)
  return {id = id, name = name, epithet = epithet or "", sex = sex, age = age, house = house, role = role, place = "ailech",
    kind = "noble", ruler = flags.ruler == true, clergy = flags.clergy == true, child = flags.child == true, importance = importance}
end
-- The first tab's values, with one value beyond each bar's range (a skill of 25 over 20, a virtue of 9 over 7, a manner of 30 over 20).
local function record_of(c, shift)
  local record = {}
  for key, value in pairs(c) do record[key] = value end
  record.body = keyed(BODY, {172 + shift, 74, 9, 13, 11, 12, 7, 10, 6})
  record.humours = keyed(HUMOURS, {4, 3, 6, 10})
  -- A skill at 0 has an empty bar (a quad has no width to draw), and the second revision's character is not the first's.
  record.skills = keyed(SKILLS, {11, 2, 0, 6, 8, 7, 9, 8, 10, 1, 25})
  record.virtue = keyed(VIRTUE, {-2, 3, 1, 2, -1, -3, 9})
  record.stress, record.threshold = 38 + shift, 45
  record.manner = keyed(MANNER, {4, 12, 3, -6, -8, -2, -10, -12, 30})
  record.standing = keyed({"popularity", "fame", "honour"}, {34, -61, 48})
  record.traits = {
    {key = "long_lived", words = "long lived", attribute = false, letter = "C"},
    {key = "cele_de", words = "C\u{E9}le D\u{E9}", attribute = true, letter = ""},
    {key = "cattle_raider", words = "cattle raider", attribute = true, letter = "B"},
  }
  record.health = HEALTH or {}
  return record
end
local niall = card(1, "Niall mac Fergaile", "m", 50, "U\u{ED} N\u{E9}ill \u{B7} Cen\u{E9}l nE\u{F3}gain", "ruler of Grian\u{E1}n Ailigh",
  {ruler = true}, 0, "Frossach")
local wife = card(2, "Dunlaith", "f", 41, "U\u{ED} N\u{E9}ill \u{B7} Cen\u{E9}l nE\u{F3}gain", "wife of the ruler of Grian\u{E1}n Ailigh", {}, 1)
local abbot = card(3, "Fothad", "m", 60, "", "abbot of Armagh", {clergy = true}, 2)
local boy = card(4, "Aed", "m", 9, "U\u{ED} N\u{E9}ill \u{B7} Cen\u{E9}l nE\u{F3}gain", "son of the ruler of Grian\u{E1}n Ailigh", {child = true}, 3)
local peasant = card(5, "Gormlaith", "f", 30, "", "peasant", {}, 4)
CH = {
  [1] = {niall, wife, abbot, boy, peasant},
  -- The second revision: a year on (Niall is 51) and Aed is dead.
  [2] = {card(1, "Niall mac Fergaile", "m", 51, niall.house, niall.role, {ruler = true}, 0, "Frossach"), wife, abbot, peasant},
  [3] = {niall, wife, abbot, boy, peasant},
}
local function record_at(id, revision)
  local list = CH[revision]
  if list == nil then
    if revision < 9 then error("game.character: revision " .. revision .. " is missing") end
    error("game.character: revision " .. revision .. " is retired")
  end
  for _, c in ipairs(list) do
    if c.id == id then return record_of(c, revision - 1) end
  end
  error("game.character: no character " .. id .. " at revision " .. revision)
end
character_calls = {}
game.characters = function(revision)
  if CH[revision] == nil then error("game.characters: revision " .. revision .. " is missing") end
  return CH[revision]
end
game.character = function(id, revision)
  character_calls[#character_calls + 1] = id .. "@" .. revision
  return record_at(id, revision)
end
-- A time with a world number.
function ctime(hour, revision, world)
  callbacks.time({year = 768, month = 1, day = 1, hour = hour, paused = false, speed = 1, economy_revision = revision, world = world})
end

-- What is drawn since `forget_drawn`: the texts (with colour and font) and the rectangles.
drawn_texts, drawn_rects = {}, {}
function forget_drawn() drawn_texts, drawn_rects = {}, {} end
do
  local draw_library = require("lib.draw")
  local real_text, real_rect = draw_library.text, draw_library.rect
  draw_library.text = function(result, group, x, y, content, colour, font_name, align)
    local shown = content
    if type(content) == "table" then
      shown = ""
      for _, segment in ipairs(content) do shown = shown .. segment.text end
    end
    drawn_texts[#drawn_texts + 1] = {text = shown, colour = colour, font = font_name or "standard", x = x, y = y}
    return real_text(result, group, x, y, content, colour, font_name, align)
  end
  draw_library.rect = function(result, group, x, y, width, height, colour)
    drawn_rects[#drawn_rects + 1] = {x = x, y = y, width = width, height = height, colour = colour}
    return real_rect(result, group, x, y, width, height, colour)
  end
  local screen_library = require("lib.character_screen")
  local real_new = screen_library.new
  screen_library.new = function(spec)
    screen_object = real_new(spec)
    return screen_object
  end
  local cards_library = require("lib.cards")
  local real_cards_new = cards_library.new
  cards_library.new = function(spec)
    local created = real_cards_new(spec)
    if spec.window.title == "ledger" then ledger_stack = created end
    if spec.window.title == "character" then screen_stack = created end
    return created
  end
  local character_cards = require("lib.character_cards")
  local real_items = character_cards.items
  character_cards.items = function(rows, on_open, world)
    shown_names = {}
    for index, row in ipairs(rows) do shown_names[index] = row.character.name end
    return real_items(rows, on_open, world)
  end
end
do
  local listbar_library = require("lib.listbar")
  local real_listbar_new = listbar_library.new
  listbar_library.new = function(spec)
    local created = real_listbar_new(spec)
    if spec.window.title == "ledger" then ledger_listbar = created end
    return created
  end
  -- The tooltips ask the shipped glossary (the game's terms and the character terms) by the key of the quantity: the keys are recorded and the
  -- answer is the real one, nil for a key it has no term for.
  local tooltip_library = require("lib.tooltip")
  local real_explanation = tooltip_library.explanation
  explained = {}
  tooltip_library.explanation = function(key)
    explained[#explained + 1] = key
    return real_explanation(key)
  end
end
HEALTH = nil
local function has_text(content)
  for _, entry in ipairs(drawn_texts) do
    if entry.text == content then return entry end
  end
  return nil
end
function text_drawn(content) return has_text(content) ~= nil end
function announce_cards()
  note("cards " .. table.concat(shown_names or {}, ",") .. " count=" .. tostring(ledger_listbar.count.ck_test_text))
  local ledger = window_objects.ledger
  note("ledger_tab_count " .. (ledger_stack ~= nil and not ledger_stack.closed and #ledger_stack.items or -1))
end
function announce_ledger_origin()
  local x, y = window_objects.ledger:content_origin()
  note(string.format("ledger_origin %g %g", x, y))
end
function announce_screen()
  local screen = screen_object
  if screen == nil or not screen:is_open() then
    note("screen closed")
    return
  end
  local x, y = screen.window:position()
  local width, height = screen.window:size()
  note(string.format("screen id=%d tab=%s at %g %g size %g %g stack_items=%d scroll=%g world=%d", screen:id(), screen:current_tab(), x, y,
    width, height, #screen_stack.items, screen_stack:scroll_position(), screen.world))
end
-- The tooltip of the section `index` of the screen's stack at the point (x, y) of UI pixels in it: the glossary key it asked for, or none.
function announce_tip(index, x, y)
  explained = {}
  local content = screen_stack.items[index].tooltip(x, y)
  note("tip " .. index .. " " .. table.concat(explained, ",") .. " " .. tostring(content ~= nil))
end
-- The body tile `n` (1-based, four to a row) of the first section: its tooltip.
function announce_body_tip(n)
  local pitch = (screen_object.window.content_width // 2 - 5) // 4
  announce_tip(1, ((n - 1) % 4) * pitch + 6, 10 + ((n - 1) // 4) * 13 + 5)
end
function announce_inner()
  note("inner " .. (screen_object.window.content_width // 2 - 5))
end
function announce_panel_geometry()
  local x, y = panel_window:position()
  local width, height = panel_window:size()
  note(string.format("panel_slot %g %g %g %g", x, y, width, height))
end
function announce_section_heights()
  local heights = {}
  for index, item in ipairs(screen_stack.items) do heights[index] = item.height end
  note("section_heights " .. table.concat(heights, ","))
end
function announce_stack_origin()
  local x, y = screen.window:content_origin()
  note(string.format("stack_origin %g %g %g %g", screen_stack.origin_x, screen_stack.origin_y, screen_stack.width, screen_stack.height))
end
function announce_screen_tabs()
  local tabs = screen_object.tabs
  for index, item in ipairs(tabs.items) do
    local button = tabs.buttons[index]
    local width, height = button:size()
    local ox, oy = screen_object.window:content_origin()
    note(string.format("screen_tab %s %g %g selected=%s", item.key, ox + button.x + width / 2, oy + button.y + height / 2,
      tostring(button.selected == true)))
  end
end
-- The widest fill of a bar of `height` 3 drawn, against its track: no fill is wider than the track it lies on.
function announce_bars_clamped()
  local widest_fill, overflow = 0, false
  for _, fill in ipairs(drawn_rects) do
    if fill.height == 3 and fill.colour == "#ece3cb" then
      for _, track in ipairs(drawn_rects) do
        if track.height == 3 and track.colour == "#3a3326" and track.y == fill.y and track.x == fill.x then
          widest_fill = math.max(widest_fill, fill.width)
          if fill.width > track.width then overflow = true end
        end
      end
    end
  end
  note("bar_overflow " .. tostring(overflow) .. " widest=" .. widest_fill)
end
"##;

fn characters_rig(steps: &[&str]) -> Rig {
    rig_with_prelude_at(CHARACTERS, steps, 1800, 900, 2)
}

/// The notes these tests made (the rig records thousands of others), for the message of a failed assertion.
fn mine(rig: &Rig) -> Vec<String> {
    const OURS: [&str; 19] = [
        "listed",
        "screen",
        "cards",
        "section_heights",
        "header",
        "numbers",
        "chips",
        "bar_overflow",
        "tip",
        "later",
        "calls",
        "new_height",
        "after_retired",
        "tall",
        "inner",
        "panel_slot",
        "same_window",
        "shows",
        "rows",
    ];
    notes(rig)
        .into_iter()
        .filter(|line| OURS.iter().any(|prefix| line.starts_with(prefix)))
        .collect()
}

fn note_line(rig: &Rig, prefix: &str) -> String {
    notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no note `{prefix}`: {:?}", mine(rig)))
}

fn click(rig: &mut Rig, x: f64, y: f64) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

/// The point at the middle of the `n`th (1-based) card of the ledger, which is 40 pixels tall with 3 between.
fn card_point(rig: &Rig, n: usize) -> (f64, f64) {
    let line = note_line(rig, "ledger_origin ");
    let mut values = line["ledger_origin ".len()..].split_whitespace();
    let (x, y): (f64, f64) = (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    );
    let n = f64::from(u32::try_from(n).expect("a small number"));
    (x + 100.0, y + ((n - 1.0) * 43.0 + 20.0) * 2.0)
}

fn open_characters(rig: &mut Rig) {
    rig.opened();
    press_bar(rig, "characters");
    rig.opened();
}

#[test]
fn the_third_ledger_tab_lists_the_characters_in_importance_order() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger() announce_ledger_tabs()",
    ]);
    open_characters(&mut rig);
    assert_eq!(
        cards_note(&rig),
        "cards Niall mac Fergaile,Dunlaith,Fothad,Aed,Gormlaith count=",
        "most important first"
    );
    let lines = notes(&rig);
    assert!(lines.iter().any(|line| line == "ledger_tab_count 5"));
    assert!(
        lines.iter().any(|line| line.contains("tab=characters")),
        "the ledger shows its third tab"
    );
}

fn cards_note(rig: &Rig) -> String {
    note_line(rig, "cards ")
}

#[test]
fn each_filter_and_sort_of_the_characters_tab_works_through_the_list_bar() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards()",
        "ledger_listbar:toggle(1, 'women') announce_cards()",
        "ledger_listbar:toggle(1, 'women') ledger_listbar:toggle(1, 'children') announce_cards()",
        "ledger_listbar:clear() ledger_listbar:toggle(1, 'clergy') announce_cards()",
        "ledger_listbar:clear() ledger_listbar:toggle(1, 'rulers') announce_cards()",
        "ledger_listbar:clear() ledger_listbar:toggle(1, 'men') announce_cards()",
        "ledger_listbar:clear() ledger_listbar:choose(2) announce_cards()",
        "ledger_listbar:choose(3) announce_cards()",
        "ledger_listbar:choose(4) announce_cards()",
        "ledger_listbar:choose(1) announce_cards()",
    ]);
    open_characters(&mut rig);
    // Nothing ticked by default: everyone, the most important first, and no count (the bar counts only a filter).
    assert_eq!(
        cards_note(&rig),
        "cards Niall mac Fergaile,Dunlaith,Fothad,Aed,Gormlaith count="
    );
    let expected = [
        // women
        "cards Dunlaith,Gormlaith count=2 of 5",
        // children
        "cards Aed count=1 of 5",
        // clergy: a monk pop or the high_cleric office
        "cards Fothad count=1 of 5",
        // rulers
        "cards Niall mac Fergaile count=1 of 5",
        // men
        "cards Niall mac Fergaile,Fothad,Aed count=3 of 5",
        // name, ascending, folding the acute vowels
        "cards Aed,Dunlaith,Fothad,Gormlaith,Niall mac Fergaile count=",
        // age: its first direction is descending
        "cards Fothad,Niall mac Fergaile,Dunlaith,Gormlaith,Aed count=",
        // house: the same house keeps the importance order, and no house comes last
        "cards Niall mac Fergaile,Dunlaith,Aed,Fothad,Gormlaith count=",
        // importance again
        "cards Niall mac Fergaile,Dunlaith,Fothad,Aed,Gormlaith count=",
    ];
    for want in expected {
        rig.opened();
        assert_eq!(cards_note(&rig), want);
    }
}

#[test]
fn the_cards_draw_the_name_house_age_and_role_and_a_tooltip_tells_them_and_nothing_more() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() local tip = ledger_stack.items[1].tooltip() note('tip_title ' .. tip.title) note('tip_lines ' .. table.concat(tip, '|')) local none = ledger_stack.items[3].tooltip() note('tip_none ' .. none.title .. '|' .. table.concat(none, '|'))",
    ]);
    rig.opened();
    // The cards are drawn as the ledger opens, so the texts are in what was drawn since the script began.
    press_bar(&mut rig, "characters");
    rig.opened();
    let lines = notes(&rig);
    assert!(
        lines.contains(&"tip_title Niall mac Fergaile Frossach".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(
            &"tip_lines U\u{ED} N\u{E9}ill \u{B7} Cen\u{E9}l nE\u{F3}gain|ruler of Grian\u{E1}n Ailigh"
                .to_owned()
        ),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"tip_none Fothad|no house|abbot of Armagh".to_owned()),
        "a character with no house says so: {lines:?}"
    );
}

#[test]
fn a_click_on_a_card_opens_the_screen_over_the_left_slot_a_second_replaces_it_and_close_returns_to_the_panel()
 {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "select_place('rowan', 'Rowan') announce_panel_geometry()",
        "announce_cards() announce_ledger_origin() announce_panel_geometry() forget_drawn()",
        "announce_screen() first_screen_window = screen_object.window announce_panel_geometry() note('shows ' .. tostring(text_drawn('Niall mac Fergaile Frossach')) .. tostring(text_drawn('age ')))",
        "announce_screen() note('same_window ' .. tostring(first_screen_window == screen_object.window) .. ' closed=' .. tostring(first_screen_window.closed))",
        "announce_screen() announce_panel_geometry()",
    ]);
    rig.opened();
    // The location panel is open over the left slot.
    rig.opened();
    let panel = note_line(&rig, "panel_slot ");
    press_bar(&mut rig, "characters");
    rig.opened();
    let (x, y) = card_point(&rig, 1);
    click(&mut rig, x, y);
    rig.opened();
    // The screen fills the same slot, and shows the character the card named.
    assert!(
        note_line(&rig, "screen id=1 tab=character at 0 ").contains("scroll=0 world=0"),
        "{:?}",
        mine(&rig)
    );
    let slot: Vec<&str> = panel["panel_slot ".len()..].split_whitespace().collect();
    let screen = note_line(&rig, "screen id=1");
    assert!(
        screen.contains(&format!(
            "at {} {} size {} {}",
            slot[0], slot[1], slot[2], slot[3]
        )),
        "the screen is the panel's rectangle: {panel} / {screen}"
    );
    // A second card replaces the screen: one window, the same one, now showing the other character.
    let (x, y) = card_point(&rig, 2);
    click(&mut rig, x, y);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("screen id=2 tab=character")),
        "{:?}",
        mine(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "same_window true closed=false"),
        "{:?}",
        mine(&rig)
    );
    // Escape closes the top window, the screen, and the location panel is as it was.
    assert!(rig.escape(), "Escape closes the screen");
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "screen closed"),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(
        note_line(&rig, "panel_slot "),
        panel,
        "the panel under it is as it was"
    );
}

#[test]
fn the_screen_header_and_the_character_tab_lay_out_every_section_with_values_beyond_range_shown_true()
 {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin() forget_drawn()",
        "announce_screen() announce_section_heights() announce_inner() \
         note('header ' .. tostring(text_drawn('Niall mac Fergaile Frossach')) .. ' ' .. tostring(text_drawn('ruler of Grian\u{E1}n Ailigh')) \
           .. ' ' .. tostring(text_drawn('U\u{ED} N\u{E9}ill \u{B7} Cen\u{E9}l nE\u{F3}gain')) .. ' ' .. tostring(text_drawn('age 50')) .. ' ' .. tostring(text_drawn('50')))",
        "screen_stack:set_pos(120)",
        "screen_stack:set_pos(400)",
        "announce_bars_clamped() note('numbers ' .. tostring(text_drawn('25')) .. tostring(text_drawn('+9')) .. tostring(text_drawn('+30')) .. tostring(text_drawn('-61')) .. tostring(text_drawn('38')) .. tostring(text_drawn('45')) .. tostring(text_drawn('173')))",
        "note('chips ' .. tostring(text_drawn('long lived')) .. tostring(text_drawn('cattle raider')) .. tostring(text_drawn('C\u{E9}le D\u{E9}')) .. ' none_health=' .. tostring(text_drawn('none')) .. ' sections=' .. #screen_stack.items)",
    ]);
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, 1);
    click(&mut rig, x, y);
    rig.opened();
    let heights = note_line(&rig, "section_heights ");
    // body, health (none), skills, virtue, temper, manner, standing, then the traits' chips (one row or two).
    assert!(
        heights.starts_with("section_heights 47,20,70,50,41,60,22,"),
        "{heights}"
    );
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line == "header true true true true false"),
        "{:?}",
        mine(&rig)
    );
    // Scrolling draws the sections that were out of the view; the values beyond a bar's range are drawn at the end and shown true.
    rig.opened();
    rig.opened();
    rig.opened();
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line.starts_with("bar_overflow false")),
        "no bar is filled past its track: {:?}",
        mine(&rig)
    );
    // 25 (a skill of 20), +9 (a virtue of 7), +30 (a manner of 20), a negative standing, the stress and its threshold; 173 is not there (that
    // is the second revision's height).
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line == "numbers truetruetruetruetruetruefalse"),
        "{:?}",
        mine(&rig)
    );
    rig.opened();
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line.starts_with("chips truetruetrue none_health=true sections=8")),
        "{:?}",
        mine(&rig)
    );
}

#[test]
fn the_health_section_draws_a_row_for_each_injury_against_a_fixture() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin() \
         HEALTH = {{kind = 'cut', part = 'left arm', severity = 1}, {kind = 'bruise', part = 'chest', severity = 2, healing = 60}} \
         forget_drawn()",
        "announce_section_heights() note('rows ' .. tostring(text_drawn('left arm')) .. tostring(text_drawn('lasting')) .. tostring(text_drawn('chest')) .. tostring(text_drawn('60%')) .. ' none=' .. tostring(text_drawn('none')))",
    ]);
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, 1);
    click(&mut rig, x, y);
    rig.opened();
    let lines = notes(&rig);
    // Two injuries are two rows of 10 under the heading's 10.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("section_heights 47,30,")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "rows truetruetruetrue none=false"),
        "{lines:?}"
    );
}

fn screen_tab_point(rig: &Rig, key: &str) -> (f64, f64) {
    let prefix = format!("screen_tab {key} ");
    let line = note_line(rig, &prefix);
    let mut values = line[prefix.len()..].split_whitespace();
    (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    )
}

#[test]
fn the_four_tabs_switch_and_the_three_not_built_yet_say_so() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin()",
        "announce_screen() announce_screen_tabs() screen_stack:set_pos(40) announce_screen() forget_drawn()",
        "announce_screen() note('later ' .. tostring(text_drawn('in a later slice')) .. ' ' .. tostring(text_drawn('nothing yet'))) forget_drawn()",
        "announce_screen() note('later ' .. tostring(text_drawn('in a later slice')) .. ' ' .. tostring(text_drawn('nothing yet')))",
        "announce_screen()",
    ]);
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, 1);
    click(&mut rig, x, y);
    rig.opened();
    // The four tabs in order, the first selected, in the window's body below the header, not in its title bar.
    let tabs: Vec<String> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("screen_tab "))
        .collect();
    let keys: Vec<&str> = tabs
        .iter()
        .map(|line| line.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(keys, ["character", "pop", "relations", "actions"]);
    assert!(tabs[0].ends_with("selected=true") && tabs[1].ends_with("selected=false"));
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line.contains("tab=character") && line.contains("scroll=40")),
        "{:?}",
        mine(&rig)
    );
    // Pressing "pop" shows its line, with the scroll back at the top; each tab is selectable.
    let (x, y) = screen_tab_point(&rig, "pop");
    click(&mut rig, x, y);
    rig.opened();
    let screen = note_line(&rig, "screen id=1");
    assert!(
        screen.contains("tab=pop") && screen.contains("stack_items=1 scroll=0"),
        "{screen}"
    );
    assert!(
        mine(&rig).iter().any(|line| line == "later true false"),
        "{:?}",
        mine(&rig)
    );
    let (x, y) = screen_tab_point(&rig, "actions");
    click(&mut rig, x, y);
    rig.opened();
    assert!(note_line(&rig, "screen id=1").contains("tab=actions"));
    assert!(
        mine(&rig).iter().any(|line| line == "later false true"),
        "{:?}",
        mine(&rig)
    );
    let (x, y) = screen_tab_point(&rig, "relations");
    click(&mut rig, x, y);
    rig.opened();
    assert!(note_line(&rig, "screen id=1").contains("tab=relations"));
}

#[test]
fn every_icon_of_the_first_tab_names_its_quantity_in_a_tooltip() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin()",
        // the body: the tiles of strength, weight and height, over their numbers too
        "announce_screen() announce_body_tip(3) announce_body_tip(2) announce_body_tip(1) announce_body_tip(9)",
        // a skill, a virtue, a humour, the stress bar's icon and its numbers, a manner, a standing value
        "announce_tip(3, 3, 14) announce_tip(4, 3, 14) announce_tip(5, 3, 14) announce_tip(5, 3, 32) announce_tip(6, 3, 14) announce_tip(7, 3, 14) \
         announce_tip(3, 3, 9) announce_tip(3, 3, 19) announce_tip(3, 3, 18) announce_tip(4, 3, 9) announce_tip(5, 3, 9) announce_tip(5, 3, 30) \
         announce_tip(6, 3, 9) announce_tip(7, 3, 9)",
        // the heading and the gap tell nothing; a chip names a trait or an attribute
        "announce_tip(1, 4, 2) screen_stack:set_pos(400) announce_tip(8, 4, 14)",
    ]);
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, 1);
    click(&mut rig, x, y);
    rig.opened();
    let tips = |rig: &Rig| -> Vec<String> {
        notes(rig)
            .into_iter()
            .filter(|line| line.starts_with("tip "))
            .collect()
    };
    for want in [
        "tip 1 strength true",
        "tip 1 weight true",
        "tip 1 height true",
        "tip 1 fecundity true",
    ] {
        assert!(
            tips(&rig).contains(&want.to_owned()),
            "{want}: {:?}",
            tips(&rig)
        );
    }
    rig.opened();
    for want in [
        "tip 3 leadership true",
        "tip 4 pride_humility true",
        "tip 5 blood true",
        "tip 5 stress true",
        "tip 6 guarded_open true",
        "tip 7 popularity true",
    ] {
        assert!(
            tips(&rig).contains(&want.to_owned()),
            "{want}: {:?}",
            tips(&rig)
        );
    }
    // The icons are drawn from the row's top pixel less one: their own top edge answers for their own quantity, and the pixel just under the
    // previous row's last one is the previous row's: (3,14) leadership, (3,9) leadership, (3,19) craftsmanship, (3,18) leadership.
    let edges: Vec<String> = tips(&rig)
        .into_iter()
        .filter(|line| line.starts_with("tip 3 "))
        .collect();
    assert_eq!(
        edges,
        [
            "tip 3 leadership true",
            "tip 3 leadership true",
            "tip 3 craftsmanship true",
            "tip 3 leadership true"
        ],
        "{edges:?}"
    );
    for want in [
        "tip 4 pride_humility true",
        "tip 5 blood true",
        "tip 5 stress true",
        "tip 6 guarded_open true",
        "tip 7 popularity true",
    ] {
        assert!(
            tips(&rig).iter().filter(|line| **line == want).count() >= 2,
            "{want} answers at the icon's top edge too: {:?}",
            tips(&rig)
        );
    }
    rig.opened();
    assert!(
        tips(&rig).contains(&"tip 1  false".to_owned()),
        "{:?}",
        tips(&rig)
    );
    assert!(
        tips(&rig).contains(&"tip 8 trait true".to_owned()),
        "{:?}",
        tips(&rig)
    );
}

#[test]
fn the_open_screen_follows_the_revisions_keeps_its_tab_and_scroll_and_closes_on_a_load() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin()",
        "screen_stack:set_pos(40) announce_screen() forget_drawn()",
        // A new revision of the same world: the character is read again from it, and the tab and scroll stay.
        "ctime(2, 2, 0) announce_screen() note('calls ' .. table.concat(character_calls, ',')) note('new_height ' .. tostring(text_drawn('173')))",
        // A read against a retired revision changes nothing; the next snapshot is read instead.
        "ctime(3, 99, 0) announce_screen() note('after_retired ' .. table.concat(character_calls, ','))",
        "ctime(4, 3, 0) announce_screen() note('calls ' .. table.concat(character_calls, ','))",
        // A load: this world has a live character with the same id, and the screen still closes.
        "CH[4] = CH[3] ctime(5, 4, 1) announce_screen()",
    ]);
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, 1);
    click(&mut rig, x, y);
    rig.opened();
    assert!(
        note_line(&rig, "screen id=1 tab=character").contains("scroll=40"),
        "{:?}",
        mine(&rig)
    );
    rig.opened();
    assert!(
        note_line(&rig, "screen id=1 tab=character").contains("scroll=40"),
        "the scroll is kept: {:?}",
        mine(&rig)
    );
    assert!(
        mine(&rig).iter().any(|line| line == "calls 1@1,1@2"),
        "{:?}",
        mine(&rig)
    );
    assert!(
        mine(&rig).iter().any(|line| line == "new_height true"),
        "the new values are drawn: {:?}",
        mine(&rig)
    );
    // The retired revision: still open, the same scroll, and the read was tried and refused.
    rig.opened();
    assert!(
        note_line(&rig, "screen id=1 tab=character").contains("scroll=40"),
        "{:?}",
        mine(&rig)
    );
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line == "after_retired 1@1,1@2,1@99"),
        "{:?}",
        mine(&rig)
    );
    // The next snapshot is read instead.
    rig.opened();
    assert!(
        mine(&rig)
            .iter()
            .any(|line| line == "calls 1@1,1@2,1@99,1@3"),
        "{:?}",
        mine(&rig)
    );
    // A new world closes it.
    rig.opened();
    assert_eq!(note_line(&rig, "screen "), "screen closed");
}

#[test]
fn a_character_who_is_no_longer_live_closes_the_screen() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards() announce_ledger_origin()",
        "announce_screen()",
        // Aed (4) is not in revision 2's list: he has died.
        "ctime(2, 2, 0) announce_screen()",
    ]);
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, 4);
    click(&mut rig, x, y);
    rig.opened();
    assert!(note_line(&rig, "screen id=4").contains("tab=character"));
    rig.opened();
    assert_eq!(note_line(&rig, "screen "), "screen closed");
}

#[test]
fn the_ledger_tab_refreshes_with_a_new_revision_keeping_its_choices() {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards()",
        "ledger_listbar:toggle(1, 'men') announce_cards()",
        "ctime(2, 2, 0) announce_cards()",
    ]);
    open_characters(&mut rig);
    rig.opened();
    assert_eq!(
        cards_note(&rig),
        "cards Niall mac Fergaile,Fothad,Aed count=3 of 5"
    );
    rig.opened();
    // Aed has died and the box stays ticked: two of the four who are left.
    assert_eq!(
        cards_note(&rig),
        "cards Niall mac Fergaile,Fothad count=2 of 4"
    );
}

#[test]
fn at_the_smallest_window_the_character_tab_is_taller_than_the_slot_and_scrolls() {
    let mut rig = rig_with_prelude_at(
        CHARACTERS,
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_cards() announce_ledger_origin()",
            "announce_screen() note('tall ' .. tostring(screen_stack.content_height > screen_stack.height) .. ' ' .. screen_stack.content_height .. ' ' .. screen_stack.height)",
        ],
        1280,
        800,
        2,
    );
    rig.opened();
    press_bar(&mut rig, "characters");
    rig.opened();
    let line = note_line(&rig, "ledger_origin ");
    let mut values = line["ledger_origin ".len()..].split_whitespace();
    let (x, y): (f64, f64) = (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    );
    rig.feed(&[
        PointerEvent::Down(points(x + 100.0, y + 40.0)),
        PointerEvent::Up(points(x + 100.0, y + 40.0)),
    ]);
    rig.deliver_fixture_frame_at(1280, 800, 2);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("tall true ")),
        "{:?}",
        mine(&rig)
    );
}

fn assets_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

#[test]
fn every_icon_the_screen_draws_is_in_the_approved_set_and_shipped_with_its_provenance() {
    let mut rig = characters_rig(&[
        "local screen = require('lib.character_screen') note('icons ' .. table.concat(screen.ICONS, ',')) note('portraits ' .. table.concat(screen.PORTRAITS, ','))",
    ]);
    rig.opened();
    let names = |prefix: &str| -> Vec<String> {
        note_line(&rig, prefix)[prefix.len()..]
            .split(',')
            .map(str::to_owned)
            .collect()
    };
    let used = names("icons ");
    let portraits = names("portraits ");
    // The approved set the generator holds: the names the screen draws are exactly its drawings (nothing used is outside it, nothing shipped
    // is unused), each written as a PNG beside it.
    let output = std::process::Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg("import runpy,sys; print('\\n'.join(sorted(runpy.run_path(sys.argv[1])['ART'])))")
        .arg(assets_dir().join("ui/characters/make_characters.py"))
        .output()
        .expect("python3 reads the generator");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let approved: Vec<String> = String::from_utf8(output.stdout)
        .expect("text")
        .lines()
        .map(str::to_owned)
        .collect();
    let mut used_sorted = used.clone();
    used_sorted.sort();
    assert_eq!(
        used_sorted, approved,
        "the screen draws the approved set, no more and no less"
    );
    for name in used.iter().chain(&portraits) {
        let path = assets_dir().join(format!("ui/characters/{name}.png"));
        assert!(path.exists(), "{} is shipped", path.display());
    }
    // Provenance lines: the generator's note for the colour set and the unknown portrait, and the monochrome bust's line in make_theme.py.
    let generator = std::fs::read_to_string(assets_dir().join("ui/characters/make_characters.py"))
        .expect("the generator");
    assert!(
        generator.contains("Provenance:") && generator.contains("SHA-256"),
        "the colour set cites its source"
    );
    assert!(
        generator.contains("The unknown portrait"),
        "the portrait says where it came from"
    );
    let theme = std::fs::read_to_string(assets_dir().join("ui/theme/make_theme.py"))
        .expect("make_theme.py");
    let bust = theme.find("\"characters\": (").expect("the bust icon");
    assert!(
        theme[..bust]
            .rsplit('\n')
            .take(4)
            .any(|line| line.contains("Drawn from plain shapes")),
        "the monochrome bust carries a provenance line"
    );
}

#[test]
fn a_release_routed_before_a_refresh_acts_for_the_card_that_was_pressed_never_for_the_one_that_moved_in()
 {
    let mut rig = characters_rig(&[
        "ctime(1, 1, 0) announce_bars()",
        "announce_cards()",
        // Aed's card (the fourth) is pressed; revision 2 then replaces the cards (Aed has died, Gormlaith is now the fourth); the release that was
        // routed against the old frame arrives after the refresh.
        "local pointer = require('lib.pointer') for key in pairs(ledger_stack.item_keys) do if key:match('_4$') then stale_key = key end end \
         stale_widget = pointer.widget(stale_key) stale_widget:on_event({kind = 'down'}, true) \
         ctime(2, 2, 0) stale_widget:on_event({kind = 'up', inside = true}, true) announce_screen() note('stale_calls ' .. table.concat(character_calls, ','))",
        // The same across a load: Niall's card is pressed in world 0; world 1 has a live character with the same number.
        "local pointer = require('lib.pointer') for key in pairs(ledger_stack.item_keys) do if key:match('_1$') then stale_key = key end end \
         stale_widget = pointer.widget(stale_key) stale_widget:on_event({kind = 'down'}, true) \
         CH[4] = CH[3] ctime(3, 4, 1) stale_widget:on_event({kind = 'up', inside = true}, true) announce_screen() note('stale_calls ' .. table.concat(character_calls, ','))",
    ]);
    open_characters(&mut rig);
    rig.opened();
    // Aed was pressed; the release acted for Aed, who is not live in revision 2: nothing opened, and Gormlaith was never read.
    assert_eq!(
        note_line(&rig, "screen "),
        "screen closed",
        "{:?}",
        mine(&rig)
    );
    let calls = note_line(&rig, "stale_calls");
    assert!(calls.contains("4@2") && !calls.contains("5@"), "{calls}");
    rig.opened();
    // Niall's card of the old world opens no one in the new world, though the number is live there.
    assert_eq!(
        note_line(&rig, "screen "),
        "screen closed",
        "{:?}",
        mine(&rig)
    );
    assert!(
        !note_line(&rig, "stale_calls").contains("1@4"),
        "{:?}",
        mine(&rig)
    );
}

/// The shipped UI over **real** data (CK-375 witness): `cards` is the Lua form of what `game.characters(1)` returned from the Rust view of the shipped 768
/// start and `record` that of `game.character(id, 1)`. The card of importance `rank` is clicked, and the notes of what the screen then showed return.
pub(crate) fn real_data_notes(cards: &str, record: &str, id: i64, rank: usize) -> Vec<String> {
    let prelude = format!(
        "{CHARACTERS}\nREAL_CARDS = {cards}\nREAL_RECORD = {record}\nCH = {{[1] = REAL_CARDS}}\n\
         game.characters = function(revision) if revision ~= 1 then error('game.characters: revision ' .. revision .. ' is missing') end return REAL_CARDS end\n\
         game.character = function(id, revision)\n\
           if id ~= {id} or revision ~= 1 then error('game.character: no character ' .. id .. ' at revision ' .. revision) end\n\
           return REAL_RECORD\n\
         end\n"
    );
    let mut rig = rig_with_prelude_at(
        &prelude,
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_cards() announce_ledger_origin()",
            "announce_screen() announce_section_heights() \
             note('header ' .. tostring(text_drawn(REAL_RECORD.name .. ' ' .. REAL_RECORD.epithet)) .. ' ' .. tostring(text_drawn(REAL_RECORD.house)) \
               .. ' ' .. tostring(text_drawn('age ' .. REAL_RECORD.age)) .. ' ' .. tostring(text_drawn(REAL_RECORD.role)))\
             note('listed ' .. #REAL_CARDS)",
        ],
        1800,
        900,
        2,
    );
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, rank + 1);
    click(&mut rig, x, y);
    rig.opened();
    mine(&rig)
}
