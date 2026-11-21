//! The character screen's relations tab (CK-377): the shipped `scripts/ui/main.lua` in a real UI VM over a stub `game` whose
//! `game.character_relations` serves fixture records, with what the tab draws recorded as it draws it.
//!
//! The fixtures are records as the simulation answers them: the people, with their relation, tie, strands and opinion, the dead with how they
//! ended. A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use super::characters::{
    CHARACTERS, card_point, click, note_line, open_characters, screen_tab_point,
};
use super::*;

/// What the relations tests add to the characters' prelude: the records of two characters and one with sixteen others, `game.character_relations`
/// over them, and observers of the tab's portraits.
pub(super) const RELATIONS: &str = r#"
local function person(id, name, sex, relation, tie, live, extra)
  local p = {id = id, name = name, epithet = "", sex = sex, live = live, age = live and 30 or 0, relation = relation, tie = tie, lines = {}}
  for key, value in pairs(extra or {}) do p[key] = value end
  return p
end
local father = person(10, "Fergal mac Maele Duin", "m", "father", "kin", false,
  {ended = "died", year = 722, month = 3, day = 4, lines = {{kind = "rival", strength = 2, side = "both", history = true}}})
local mother = person(5, "Gormlaith", "f", "mother", "kin", true, {age = 73, opinion = 3, opinion_back = 5})
local eithne = person(12, "Eithne", "f", "wife", "", false, {ended = "before", year = 768})
local dunlaith = person(2, "Dunlaith", "f", "wife", "kin", true, {age = 41, opinion = 8, opinion_back = 6, lines = {
  {kind = "love", strength = 5, side = "mine", history = false}, {kind = "respect", strength = 2, side = "theirs", history = false}}})
local aed = person(4, "Aed", "m", "son", "kin", true, {age = 9, opinion = 6, opinion_back = 7})
local lost = person(13, "Indrechtach", "m", "son", "kin", false, {ended = "died", year = 760, month = 1, day = 9})
local donnchad = person(20, "Donnchad", "m", "rival", "", true, {epithet = "Midi", opinion = -9, opinion_back = 2, lines = {
  {kind = "rival", strength = 6, side = "both", history = false}, {kind = "friend", strength = 3, side = "both", history = false},
  {kind = "respect", strength = 4, side = "mine", history = false}, {kind = "hatred", strength = 7, side = "theirs", history = false}}})
local fosterling = person(21, "Coirpre", "m", "foster-son", "foster", true, {age = 16, opinion = 7, opinion_back = 10})
local hostage = person(22, "Sechnasach", "m", "hostage", "hostage", true, {age = 8, opinion = 4, opinion_back = 5})
local cousin = person(23, "Fiachnae", "m", "brother", "kin", true, {age = 71, opinion = 1, opinion_back = 1})
REL = {
  [1] = {
    id = 1, spouse_word = "wives", father = father, mother = mother, spouses = {eithne, dunlaith}, children = {aed, lost},
    others = {donnchad, fosterling, hostage, cousin},
    passions = {
      {kind = "loyalty", strength = 8, target_kind = "house", key = "cenel_neogain", name = "C\u{E9}n\u{E9}l nE\u{F3}gain"},
      {kind = "love", strength = 5, target_kind = "place", key = "iona", name = "Iona"},
      {kind = "hatred", strength = 6, target_kind = "culture", key = "food_days", name = ""},
      {kind = "fear", strength = 3, target_kind = "religion", key = "building_land", name = ""},
      {kind = "love", strength = 2, target_kind = "culture", key = "no_such_term", name = ""},
    },
  },
  -- Dunlaith: nothing but a husband: every other section is left out.
  [2] = {id = 2, spouse_word = "husbands", spouses = {person(1, "Niall", "m", "husband", "kin", true, {age = 50, opinion = 2, opinion_back = 2})},
    children = {}, others = {}, passions = {}},
  -- Fothad: sixteen others, which do not fit in two rows.
  [3] = {id = 3, spouse_word = "wives", spouses = {}, children = {}, passions = {}, others = {}},
}
for index = 1, 16 do
  REL[3].others[index] = person(100 + index, "Friend " .. index, "m", "friend", "", true,
    {lines = {{kind = "friend", strength = 1, side = "both", history = false}}, opinion = index, opinion_back = -index})
end
relation_calls = {}
game.character_relations = function(id, revision)
  relation_calls[#relation_calls + 1] = id .. "@" .. revision
  if CH[revision] == nil then
    if revision < 9 then error("game.character_relations: revision " .. revision .. " is missing") end
    error("game.character_relations: revision " .. revision .. " is retired")
  end
  local record = REL[id]
  if record == nil or REL_GONE == true then
    error("game.character_relations: no character " .. id .. " at revision " .. revision)
  end
  return record
end

-- What images are drawn since `forget_drawn`.
drawn_images = {}
do
  local draw_library = require("lib.draw")
  local real_image, real_forget = draw_library.image, forget_drawn
  draw_library.image = function(result, group, name, x, y, width, height, colour)
    drawn_images[#drawn_images + 1] = {name = name, x = x, y = y, width = width, height = height, colour = colour}
    return real_image(result, group, name, x, y, width, height, colour)
  end
  forget_drawn = function() real_forget() drawn_images = {} end
end

-- A tooltip's words on one line: the title, then each line's spans, divided by " / ".
function flat(content)
  local parts = {}
  if content.title ~= nil then parts[#parts + 1] = content.title end
  for _, line in ipairs(content) do
    if type(line) == "string" then
      parts[#parts + 1] = line
    else
      local words = ""
      for _, span in ipairs(line) do words = words .. (type(span) == "string" and span or span.text) end
      parts[#parts + 1] = words
    end
  end
  return table.concat(parts, " / ")
end

-- The items of the screen's stack: their number and heights.
function announce_rel_items()
  local heights = {}
  for index, item in ipairs(screen_stack.items) do heights[index] = item.height end
  note("rel_items " .. #screen_stack.items .. " " .. table.concat(heights, ","))
end

-- Where to press for a region of the item `index` (window points): its middle.
function middle_of(index, hit)
  local ui_library = require("lib.ui")
  local P = ui_library.P
  local x = screen_stack.origin_x + screen_stack.x + (hit.x + hit.width / 2) * P
  local y = screen_stack.origin_y + screen_stack.y - ui_library.snap(screen_stack.scroll_offset) + screen_stack.tops[index]
    + (hit.y + math.min(hit.height, 12) / 2) * P
  return x, y
end

-- The tooltip content the stack answers at the window point (x, y) of the item `index`: through the provider the stack attached to the item's
-- hit region, which is what a pointer resting there asks, and nothing else.
local function asked(index, x, y)
  local key = screen_stack.base .. "_item_" .. screen_stack.generation .. "_" .. index
  local source = tooltip_sources[key]
  if source == nil then return nil end
  return source(x, y)
end

-- The portraits (and the chip) of the item `index`, without the icons under them.
function portraits_of(index)
  local list = {}
  for _, hit in ipairs(screen_stack.items[index].hits) do
    if hit.part ~= "icon" then list[#list + 1] = hit end
  end
  return list
end

-- Every portrait of the item `index`: where to press (window points), whether a press opens, and the tooltip the stack answers there.
function announce_rel_hits(index)
  for n, hit in ipairs(portraits_of(index)) do
    local x, y = middle_of(index, hit)
    local content = asked(index, x, y)
    note(string.format("rel_hit %d %d %g %g %g %g %s | %s", index, n, x, y, hit.x, hit.y, tostring(hit.open ~= nil),
      content == nil and "<no tooltip>" or flat(content)))
  end
end

-- Every strand icon of the item `index`: the tooltip the stack answers on it.
function announce_rel_icons(index)
  for _, hit in ipairs(screen_stack.items[index].hits) do
    if hit.part == "icon" then
      local x, y = middle_of(index, hit)
      local content = asked(index, x, y)
      local terms = {}
      for _, line in ipairs(content or {}) do
        if type(line) == "table" then
          for _, span in ipairs(line) do
            if type(span) == "table" and span.term ~= nil then terms[#terms + 1] = span.term end
          end
        end
      end
      note(string.format("rel_icon %d %g %g %s | %s | %s", index, hit.x, hit.y, tostring(hit.open ~= nil),
        content == nil and "<no tooltip>" or flat(content), table.concat(terms, ",")))
    end
  end
end

-- The glossary terms the spans of the tooltip the stack answers on portrait `n` of the item `index` link, in order.
function announce_rel_terms(index, n)
  local x, y = middle_of(index, portraits_of(index)[n])
  local terms = {}
  for _, line in ipairs(asked(index, x, y) or {}) do
    if type(line) == "table" then
      for _, span in ipairs(line) do
        if type(span) == "table" and span.term ~= nil then terms[#terms + 1] = span.term end
      end
    end
  end
  note(string.format("rel_terms %d %d %s", index, n, table.concat(terms, ",")))
end

-- The names of the images drawn under portrait `n` of the item `index`, left to right ("*" for a tinted one), and a "+" when more stand.
function announce_rel_badges(index, n)
  local ui_library = require("lib.ui")
  local hit = portraits_of(index)[n]
  local top = screen_stack.tops[index] // ui_library.P
  local names, seen = {}, {}
  for _, image in ipairs(drawn_images) do
    -- A tab drawn twice (a redraw) draws its icons twice: each is told once.
    local label = image.name .. (image.colour ~= nil and "*" or "")
    if image.y == top + hit.y + 26 and image.x >= hit.x and image.x < hit.x + 24 and not seen[label] then
      seen[label] = true
      names[#names + 1] = label
    end
  end
  local plus = false
  for _, entry in ipairs(drawn_texts) do
    if entry.text == "+" and entry.y == top + hit.y + 26 and entry.x >= hit.x and entry.x < hit.x + 24 then plus = true end
  end
  note(string.format("rel_badges %d %d %s%s", index, n, table.concat(names, ","), plus and " +" or ""))
end

function announce_rel_texts(...)
  local out = {}
  for _, word in ipairs({...}) do out[#out + 1] = word .. "=" .. tostring(text_drawn(word)) end
  note("rel_texts " .. table.concat(out, " "))
end
"#;

fn relations_rig(steps: &[&str]) -> Rig {
    let prelude = format!("{CHARACTERS}{RELATIONS}");
    rig_with_prelude_at(&prelude, steps, 1800, 900, 2)
}

/// The notes these tests made.
fn mine(rig: &Rig) -> Vec<String> {
    notes(rig)
        .into_iter()
        .filter(|line| line.starts_with("rel_") || line.starts_with("screen"))
        .collect()
}

/// The `rel_hit` notes of the item `index`: the point to press, the position in the item and the tooltip.
fn hits(rig: &Rig, index: usize) -> Vec<(f64, f64, f64, f64, String)> {
    hits_with_live(rig, index)
        .into_iter()
        .map(|(x, y, rx, ry, _, tip)| (x, y, rx, ry, tip))
        .collect()
}

/// Like [`hits`], with whether a press on the portrait opens a character.
fn hits_with_live(rig: &Rig, index: usize) -> Vec<(f64, f64, f64, f64, bool, String)> {
    let prefix = format!("rel_hit {index} ");
    notes(rig)
        .into_iter()
        .filter(|line| line.starts_with(&prefix))
        .map(|line| {
            let (head, tip) = line.split_once(" | ").expect("a tooltip");
            let mut values = head[prefix.len()..].split_whitespace().skip(1);
            let mut number = || -> f64 { values.next().unwrap().parse().unwrap() };
            let (x, y, rx, ry) = (number(), number(), number(), number());
            let live = values.next().unwrap() == "true";
            (x, y, rx, ry, live, tip.to_owned())
        })
        .collect()
}

/// Opens the screen of the `n`th card of the ledger and its relations tab: after this the next step runs over the tab. The steps before it
/// are the two the rig needs to open the ledger and the one that records the tabs.
fn open_relations(rig: &mut Rig, card: usize) {
    open_characters(rig);
    let (x, y) = card_point(rig, card);
    click(rig, x, y);
    rig.opened();
    let (x, y) = screen_tab_point(rig, "relations");
    click(rig, x, y);
    rig.opened();
}

const OPEN: [&str; 3] = [
    "ctime(1, 1, 0) announce_bars()",
    "announce_cards() announce_ledger_origin()",
    "announce_screen() announce_screen_tabs() forget_drawn()",
];

fn steps_after_open<'a>(after: &[&'a str]) -> Vec<&'a str> {
    OPEN.iter().chain(after.iter()).copied().collect()
}

#[test]
fn the_tab_lays_out_its_sections_and_leaves_an_empty_one_out() {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_screen() announce_rel_items() announce_rel_texts('parents', 'wives', 'children', 'others', 'passions')",
    ]));
    open_relations(&mut rig, 1);
    // The top line, the children, the others and the passions: four sections, each headed by its word, the spouses' by "wives".
    let items = note_line(&rig, "rel_items ");
    assert!(
        items.starts_with("rel_items 4 "),
        "{items}: {:?}",
        mine(&rig)
    );
    assert_eq!(
        note_line(&rig, "rel_texts "),
        "rel_texts parents=true wives=true children=true others=true passions=true"
    );
    assert!(
        note_line(&rig, "screen id=1").contains("tab=relations"),
        "{:?}",
        mine(&rig)
    );
}

#[test]
fn a_character_with_only_a_husband_shows_the_top_line_alone_headed_husbands() {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_screen() announce_rel_items() announce_rel_texts('parents', 'wives', 'husbands', 'children', 'others', 'passions')",
    ]));
    open_relations(&mut rig, 2);
    assert!(
        note_line(&rig, "rel_items ").starts_with("rel_items 1 "),
        "every empty section is left out: {:?}",
        mine(&rig)
    );
    assert_eq!(
        note_line(&rig, "rel_texts "),
        "rel_texts parents=true wives=false husbands=true children=false others=false passions=false"
    );
}

#[test]
fn portrait_tooltips_tell_the_strands_in_order_and_the_opinion_both_ways_for_the_living_and_history_for_the_dead()
 {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_rel_hits(1) announce_rel_hits(2) announce_rel_hits(3)",
    ]));
    open_relations(&mut rig, 1);
    let top = hits(&rig, 1);
    // The father (dead), the mother, then the spouses in the stored order: Eithne (dead) and Dunlaith.
    let tips: Vec<&str> = top.iter().map(|hit| hit.4.as_str()).collect();
    assert_eq!(
        tips,
        [
            "Fergal mac Maele Duin / father / died 4 Mar 722 / rival 2 (history)",
            "Gormlaith / mother / age 73 / opinion 3 Niall toward her / opinion 5 she toward Niall",
            "Eithne / wife / died before 768",
            "Dunlaith / wife / age 41 / love 5 Niall toward her / respect 2 she toward Niall / opinion 8 Niall toward her / opinion 6 she toward Niall",
        ]
    );
    // The children: the live son has the opinion both ways; the dead one only how it ended.
    let children: Vec<String> = hits(&rig, 2).into_iter().map(|hit| hit.4).collect();
    assert_eq!(
        children,
        [
            "Aed / son / age 9 / opinion 6 Niall toward him / opinion 7 he toward Niall",
            "Indrechtach / son / died 9 Jan 760",
        ]
    );
    // The others: a rival who is also a friend, with the bonds first, then Niall's feelings, then his, and the opinion each way.
    let others: Vec<String> = hits(&rig, 3).into_iter().map(|hit| hit.4).collect();
    assert_eq!(
        others[0],
        "Donnchad Midi / rival / age 30 / rival 6 / friend 3 / respect 4 Niall toward him / hatred 7 he toward Niall / opinion -9 Niall toward him / opinion 2 he toward Niall"
    );
    assert_eq!(others.len(), 4, "{others:?}");
    // The kin and kinds beside them: foster kin, a hostage and a brother, each by its word.
    assert!(
        others[1].starts_with("Coirpre / foster-son / age 16"),
        "{}",
        others[1]
    );
    assert!(
        others[2].starts_with("Sechnasach / hostage / age 8"),
        "{}",
        others[2]
    );
    assert!(
        others[3].starts_with("Fiachnae / brother / age 71"),
        "{}",
        others[3]
    );
}

#[test]
fn a_strand_label_links_the_shared_relationship_term() {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_rel_hits(1) announce_rel_terms(1, 4) announce_rel_terms(1, 1)",
    ]));
    open_relations(&mut rig, 1);
    // Dunlaith's tooltip has two strand lines (love, respect), each linking the one `relationship` term: no per-kind terms. The dead father's
    // history line is dim text and links nothing.
    assert_eq!(
        note_line(&rig, "rel_terms 1 4 "),
        "rel_terms 1 4 relationship,relationship"
    );
    assert_eq!(note_line(&rig, "rel_terms 1 1 "), "rel_terms 1 1 ");
}

#[test]
fn a_strand_icon_has_its_own_tooltip_naming_its_kind_and_a_click_on_it_acts_for_its_portrait() {
    let mut rig = relations_rig(&steps_after_open(&["announce_rel_icons(1)"]));
    open_relations(&mut rig, 1);
    let icons: Vec<String> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("rel_icon 1 "))
        .collect();
    // The dead father's history bond and Dunlaith's love: each answers with its kind, linking the one `relationship` term, and the dead one
    // opens nothing while the live one opens its portrait's character.
    assert_eq!(icons.len(), 2, "{icons:?}");
    assert!(
        icons[0].ends_with("false | rival | relationship"),
        "{}",
        icons[0]
    );
    assert!(
        icons[1].ends_with("true | love | relationship"),
        "{}",
        icons[1]
    );
}

#[test]
fn the_tab_opens_from_the_last_completed_view_while_the_game_is_paused() {
    let mut rig = relations_rig(&[
        "callbacks.time({year = 768, month = 1, day = 1, hour = 1, paused = 'player', speed = 1, economy_revision = 1, world = 0}) announce_bars()",
        "announce_cards() announce_ledger_origin()",
        "announce_screen() announce_screen_tabs() forget_drawn()",
        "announce_screen() announce_rel_items() note('rel_calls ' .. table.concat(relation_calls, ','))",
    ]);
    open_relations(&mut rig, 1);
    // Paused, no further snapshot is coming: the tab reads at the revision of the last one delivered.
    assert!(
        note_line(&rig, "rel_items ").starts_with("rel_items 4 "),
        "{:?}",
        mine(&rig)
    );
    assert_eq!(note_line(&rig, "rel_calls "), "rel_calls 1@1");
}

#[test]
fn under_each_portrait_stand_a_dim_tie_icon_then_the_strongest_strands_and_a_plus_when_more_stand()
{
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_rel_badges(1, 1) announce_rel_badges(1, 2) announce_rel_badges(1, 3) announce_rel_badges(1, 4) announce_rel_badges(3, 1) announce_rel_badges(3, 2) announce_rel_badges(3, 3) announce_rel_badges(3, 4)",
    ]));
    open_relations(&mut rig, 1);
    let badges: Vec<String> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("rel_badges "))
        .collect();
    // Item 1: the dead father (a dim tree, then his rival strand, which is history shared: it stands as a bond); the mother (a dim tree only);
    // Eithne (a cold marriage: nothing, an empty space); Dunlaith (a dim tree, then her love: a feeling of Niall's own; hers toward him does not stand there).
    assert_eq!(
        &badges[..4],
        [
            "rel_badges 1 1 tree*,fist",
            "rel_badges 1 2 tree*",
            "rel_badges 1 3 ",
            "rel_badges 1 4 tree*,heart",
        ],
        "{badges:?}"
    );
    // The others: Donnchad has a rival, a friend and Niall's respect: the strongest two stand (rival 6, respect 4) and a "+" for the third;
    // the foster-son a dim bowl; the hostage a dim lock; the brother a dim tree.
    assert_eq!(
        &badges[4..],
        [
            "rel_badges 3 1 fist,star_pale +",
            "rel_badges 3 2 bowl*",
            "rel_badges 3 3 lock*",
            "rel_badges 3 4 tree*",
        ],
        "{badges:?}"
    );
}

#[test]
fn a_click_on_a_live_portrait_replaces_the_screen_and_one_on_a_dead_one_does_nothing() {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_rel_hits(1)",
        "announce_screen()",
        "announce_screen()",
    ]));
    open_relations(&mut rig, 1);
    let top = hits(&rig, 1);
    // The father is dead: the press lands and nothing opens.
    let (x, y, ..) = top[0];
    click(&mut rig, x, y);
    rig.opened();
    assert!(
        note_line(&rig, "screen id=").starts_with("screen id=1 tab=relations"),
        "{:?}",
        mine(&rig)
    );
    // Dunlaith is live (the fourth portrait): her screen replaces this one, opened on its first tab.
    let (x, y, ..) = top[3];
    click(&mut rig, x, y);
    rig.opened();
    assert!(
        note_line(&rig, "screen id=").starts_with("screen id=2 tab=character"),
        "{:?}",
        mine(&rig)
    );
}

#[test]
fn the_others_row_wraps_once_and_past_that_ends_in_a_chip_whose_tooltip_lists_the_rest() {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_rel_items() announce_rel_hits(2)",
    ]));
    open_relations(&mut rig, 3);
    let others = hits(&rig, 2);
    // Sixteen others: thirteen portraits in two rows of seven and the chip in the fourteenth place, which says how many were left out.
    assert_eq!(others.len(), 14, "{:?}", mine(&rig));
    let rows: std::collections::BTreeSet<String> =
        others.iter().map(|hit| hit.3.to_string()).collect();
    assert_eq!(rows.len(), 2, "two rows: {rows:?}");
    let chip = &others[13].4;
    assert_eq!(
        chip, "+3 more / Friend 14 (friend) / Friend 15 (friend) / Friend 16 (friend)",
        "the chip lists the names that did not fit, with their kind"
    );
}

#[test]
fn passion_targets_read_as_a_place_a_house_a_culture_term_a_religion_term_and_a_key_fallback() {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_rel_texts('loyalty', 'C\\u{E9}n\\u{E9}l nE\\u{F3}gain', 'love', 'Iona', 'hatred', 'food days', 'fear', 'building land', 'no_such_term')",
    ]));
    open_relations(&mut rig, 1);
    // The house by its name, the place by its name, a culture and a religion by the title of their glossary terms (the fixture's keys name two
    // terms the game glossary has), and a key no term names as the key itself.
    assert_eq!(
        note_line(&rig, "rel_texts "),
        "rel_texts loyalty=true C\u{E9}n\u{E9}l nE\u{F3}gain=true love=true Iona=true hatred=true food days=true fear=true building land=true no_such_term=true"
    );
}

/// A portrait of the tab: where to press (window points), whether a press opens a character, and its tooltip.
pub(crate) type Portrait = (f64, f64, bool, String);

/// The shipped UI over **real** data (CK-377 witness): `cards` and `record` are the Lua forms of what `game.characters(1)` and `game.character(id, 1)`
/// returned from the Rust view of the shipped 768 start, `relations` that of `game.character_relations(id, 1)`. The card of importance `rank` is
/// clicked, then the relations tab; returns the notes, the portraits `(x, y, tooltip, opens)` of the tab in order, and the screen line after a press
/// on the first (the dead father).
pub(crate) fn real_relations(
    cards: &str,
    record: &str,
    relations: &str,
    id: i64,
    rank: usize,
) -> (Vec<String>, Vec<Portrait>, String) {
    let prelude = format!(
        "{CHARACTERS}{RELATIONS}\nREAL_CARDS = {cards}\nREAL_RECORD = {record}\nREAL_RELATIONS = {relations}\nCH = {{[1] = REAL_CARDS}}\n\
         game.characters = function(revision) return REAL_CARDS end\n\
         game.character = function(id, revision)\n\
           if id ~= {id} or revision ~= 1 then error('game.character: no character ' .. id .. ' at revision ' .. revision) end\n\
           return REAL_RECORD\n\
         end\n\
         game.character_relations = function(id, revision)\n\
           if id ~= {id} or revision ~= 1 then error('game.character_relations: no character ' .. id .. ' at revision ' .. revision) end\n\
           return REAL_RELATIONS\n\
         end\n"
    );
    let mut rig = rig_with_prelude_at(
        &prelude,
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_cards() announce_ledger_origin()",
            "announce_screen() announce_screen_tabs() forget_drawn()",
            "announce_screen() announce_rel_items() for index, item in ipairs(screen_stack.items) do if item.hits ~= nil then announce_rel_hits(index) end end",
            "announce_screen()",
        ],
        1800,
        900,
        2,
    );
    open_characters(&mut rig);
    let (x, y) = card_point(&rig, rank + 1);
    click(&mut rig, x, y);
    rig.opened();
    let (x, y) = screen_tab_point(&rig, "relations");
    click(&mut rig, x, y);
    rig.opened();
    let mut portraits = Vec::new();
    for index in 1..=8 {
        for (x, y, _, _, live, tip) in hits_with_live(&rig, index) {
            portraits.push((x, y, live, tip));
        }
    }
    // A press on the first portrait (the dead father): nothing opens.
    if let Some((x, y, ..)) = portraits.first() {
        click(&mut rig, *x, *y);
    }
    rig.opened();
    let screen = note_line(&rig, "screen id=");
    (mine(&rig), portraits, screen)
}

#[test]
fn the_tab_reads_again_on_each_delivered_revision_and_closes_with_a_world_change_or_a_character_no_longer_there()
 {
    let mut rig = relations_rig(&steps_after_open(&[
        "announce_screen() note('rel_calls ' .. table.concat(relation_calls, ','))",
        "ctime(2, 2, 0)",
        "announce_screen() note('rel_calls ' .. table.concat(relation_calls, ','))",
        "REL_GONE = true ctime(3, 3, 0)",
        "announce_screen()",
    ]));
    open_relations(&mut rig, 1);
    // Opened at revision 1: the tab was read at that revision, once.
    assert!(
        note_line(&rig, "rel_calls ").ends_with("1@1"),
        "{:?}",
        mine(&rig)
    );
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    // Revision 2 is delivered: the tab shows still, and was read again at the revision delivered.
    assert!(
        note_line(&rig, "screen id=1").contains("tab=relations"),
        "{:?}",
        mine(&rig)
    );
    assert!(
        note_line(&rig, "rel_calls ").ends_with("1@1,1@2"),
        "{:?}",
        mine(&rig)
    );
    // The simulation no longer holds the character as live: the screen closes.
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "screen closed"),
        "{:?}",
        mine(&rig)
    );
}

/// Lua that presses the live portrait `n` of the item `index`, with the release kept back as the router would have routed it against this frame:
/// the widget of the item's hit key and the window point of the portrait.
const PRESS: &str = r#"
function press_portrait(index, n)
  local pointer = require("lib.pointer")
  local key = screen_stack.base .. "_item_" .. screen_stack.generation .. "_" .. index
  local hit = portraits_of(index)[n]
  local x, y = middle_of(index, hit)
  stale_widget, stale_x, stale_y = pointer.widget(key), x, y
  stale_widget:on_event({kind = "down", x = x, y = y}, true)
end
function release_stale()
  local ok, why = pcall(stale_widget.on_event, stale_widget, {kind = "up", inside = true, x = stale_x, y = stale_y}, true)
  note("rel_release " .. tostring(ok) .. " " .. tostring(why) .. " calls=" .. #character_calls)
end
"#;

#[test]
fn a_release_routed_against_an_old_frame_never_acts_in_the_new_geometry() {
    // The others section's live portrait is pressed; a same-world revision then removes the others and passions (case one) or the children (case
    // two: the section's top moves up and another takes its place); the release routed against the old frame arrives after the refresh.
    let prelude = format!("{CHARACTERS}{RELATIONS}{PRESS}");
    let mut rig = rig_with_prelude_at(
        &prelude,
        &steps_after_open(&[
            "press_portrait(3, 2) local before = #character_calls note('rel_before ' .. before) REL[1] = {id = 1, spouse_word = 'wives', spouses = REL[1].spouses, children = REL[1].children, others = {}, passions = {}} ctime(2, 2, 0)",
            "release_stale() announce_screen() announce_rel_items()",
        ]),
        1800,
        900,
        2,
    );
    open_relations(&mut rig, 1);
    rig.opened();
    // Nothing crashed, nothing opened and no character was read for the stale press.
    let release = note_line(&rig, "rel_release ");
    assert!(
        release.starts_with("rel_release true"),
        "the release is handled: {release}"
    );
    assert!(
        note_line(&rig, "screen id=").starts_with("screen id=1 tab=relations"),
        "{:?}",
        mine(&rig)
    );
}

#[test]
fn a_release_routed_against_a_section_that_moved_opens_no_one() {
    // Fothad has eight children (two rows) above sixteen others. The second other is pressed; a revision then leaves him one child, so the others
    // section's top moves up by a row; the release routed against the old frame would, in the new geometry, land on a portrait of the second row.
    let prelude = format!("{CHARACTERS}{RELATIONS}{PRESS}");
    let mut rig = rig_with_prelude_at(
        &prelude,
        &[
            OPEN[0],
            OPEN[1],
            "local kids = {} for i = 1, 8 do kids[i] = {id = 200 + i, name = 'Kid ' .. i, epithet = '', sex = 'm', live = true, age = 5, relation = 'son', tie = 'kin', lines = {}, opinion = 1, opinion_back = 1} end REL[3].children = kids announce_screen() announce_screen_tabs() forget_drawn()",
            "press_portrait(3, 2) note('rel_before ' .. #character_calls) REL[3].children = {REL[3].children[1]} ctime(2, 2, 0)",
            "release_stale() announce_screen() announce_rel_items()",
            "announce_screen() note('rel_late ' .. #character_calls)",
        ],
        1800,
        900,
        2,
    );
    open_relations(&mut rig, 3);
    rig.opened();
    // The action a release takes is deferred to the end of the next pointer dispatch: a harmless press elsewhere delivers it, and then the
    // calls made are read.
    click(&mut rig, 4.0, 4.0);
    rig.opened();
    assert!(
        note_line(&rig, "rel_release ").starts_with("rel_release true"),
        "{:?}",
        mine(&rig)
    );
    // No character was asked for by the stale release (the calls after it are the ones before it: the screen's own reads).
    let before: usize = note_line(&rig, "rel_before ")["rel_before ".len()..]
        .parse()
        .unwrap();
    let after: usize = note_line(&rig, "rel_release ")
        .rsplit("calls=")
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // The refresh itself reads the character once (the screen's own call); the release adds none.
    assert!(
        after <= before + 1,
        "before {before}, after {after}: {:?}",
        mine(&rig)
    );
    // After the next pointer dispatch (which would run a deferred open) still no character was asked for.
    let late: usize = note_line(&rig, "rel_late ")["rel_late ".len()..]
        .parse()
        .unwrap();
    assert!(
        late <= before + 1,
        "before {before}, late {late}: {:?}",
        mine(&rig)
    );
    assert!(
        note_line(&rig, "screen id=").starts_with("screen id=3 tab=relations"),
        "{:?}",
        mine(&rig)
    );
}
