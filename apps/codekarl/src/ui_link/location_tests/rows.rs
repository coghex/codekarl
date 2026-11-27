use super::*;

#[test]
fn location_rows_stay_one_line_and_open_the_exact_slot_in_a_detail_window() {
    let mut rig = rig(&[
        r"time(1, 1); select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)
look_buttons.rows.on_click()
parent_stack = card_object
assert(parent_stack.items[2].height == 13, 'building row is one line')
parent_stack.items[2].on_click(); require('lib.pointer').dispatch({})",
        r"assert(window_objects.building ~= nil and not window_objects.building.closed, 'building panel opened')
assert(parent_stack.items[2].height == 13, 'row did not expand')
window_objects.building:close()
assert(not window_objects.Cards.closed, 'closing detail returns to location')",
    ]);
    rig.opened();
    rig.deliver_fixture_frame();
    rig.opened();
}

#[test]
fn building_rows_header_uses_positions() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs(); tabs_by_title.Cards:select('buildings', true)",
        "look_buttons.rows.on_click()",
    ]);
    rig.opened();
    rig.opened();
    rig.opened();
    let captured = notes(&rig);
    assert!(card_run(&captured, "#9a927e", "positions"), "{captured:?}");
    assert!(!card_run(&captured, "#9a927e", "staff"), "{captured:?}");
}

#[test]
fn card_scroll_uses_thirteen_pixel_lines_and_shrinking_clamps_the_offset() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "local many = {}; for i = 1, 40 do many[i] = card_object.items[1] end; \
         card_object:set_items(many, 3); note('scroll0 ' .. card_object:scroll_position()); \
         card_object:scroll({amount = -1, unit = 'lines'}); note('scroll1 ' .. card_object:scroll_position()); \
         card_object:set_items({card_object.items[1]}, 3); note('scroll2 ' .. card_object:scroll_position())",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    // The third UI step reuses the same fixture card to exercise scrolling and a shrinking stack.
    let recorded = notes(&rig);
    assert!(recorded.contains(&"scroll0 0".to_owned()));
    assert!(recorded.contains(&"scroll1 26".to_owned()));
    assert!(recorded.contains(&"scroll2 0".to_owned()));
    assert!(
        rig.sink.quads().iter().any(|quad| quad.clip.is_some()),
        "the stack is clipped to the content viewport"
    );
}

#[test]
fn pop_rows_have_type_tints_and_route_to_the_place_pops_tab() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_tabs()",
        "announce_active_panel()",
    ]);
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let items = rows(&rig).last().unwrap().clone();
    assert!(items.contains("peasant ulaid@16:left:#c9932e,120@224:right:-,Rowan@228:left:-;stripe=#c9932e;icon=pops"), "{items}");
    assert!(
        items.contains("#ece3cb"),
        "unknown pop type falls back to text: {items}"
    );
    assert!(items.contains("monk mide@16:left:#4fb3c8"), "{items}");
    // The first pop belongs to Rowan; selecting it opens the location panel on its Pops tab.
    click_ledger_row(&mut rig, 1, 1800, 900);
    rig.opened();
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "active_panel Rowan pops ledger_closed=false"),
        "{:?}",
        notes(&rig)
    );
}

/// What the buildings rows drew in a window `height` points high (`cardtext x y width colour value`, `cardquad ...`), and where the first
/// building row's tooltips answer.
fn building_rows_drawing(height: u32) -> (Vec<String>, String) {
    let mut rig = rig_at(
        &[
            "time(1, 1) select_place('cards', 'Cards')",
            "announce_tabs(); tabs_by_title.Cards:select('buildings', true)",
            "note('rows_from_here') look_buttons.rows.on_click()",
            r"local P = require('lib.ui').P
local row = card_object.items[2]
local full, narrow = card_object.width // P, card_object.row_w // P
local function part(x) local content, name = row.tooltip(x, 3); return tostring(name) end
note('tip full=' .. full .. ' row=' .. narrow .. ' at_row_edge=' .. part(narrow - 4) .. ' at_full_edge=' .. part(full - 4))",
        ],
        1800,
        height,
        2,
    );
    for _ in 0..4 {
        rig.opened();
    }
    let recorded = notes(&rig);
    // Only what the rows look drew: after the switch from cards.
    let from = recorded
        .iter()
        .rposition(|line| line == "rows_from_here")
        .expect("the look switch");
    let drawn = recorded[from..]
        .iter()
        .filter(|line| line.starts_with("cardtext ") || line.starts_with("cardquad "))
        .cloned()
        .collect();
    let tip = recorded
        .iter()
        .rev()
        .find(|line| line.starts_with("tip "))
        .expect("the tooltip note")
        .clone();
    (drawn, tip)
}

fn text_x_of(drawn: &[String], value: &str) -> i64 {
    drawn
        .iter()
        .find(|line| line.starts_with("cardtext ") && line.ends_with(&format!(" {value}")))
        .unwrap_or_else(|| panic!("no text {value}: {drawn:?}"))
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn overflowing_building_rows_draw_their_header_and_numbers_5_pixels_narrower_and_their_tooltips_follow()
 {
    let (fits, fit_tip) = building_rows_drawing(900);
    let (over, over_tip) = building_rows_drawing(260);
    // The header: `building` is at the left and `positions` and `pay` are right-aligned.
    assert_eq!(text_x_of(&over, "building"), text_x_of(&fits, "building"));
    assert_eq!(
        text_x_of(&over, "positions"),
        text_x_of(&fits, "positions") - 10
    );
    assert_eq!(text_x_of(&over, "pay"), text_x_of(&fits, "pay") - 10);
    // The first building's offer label is right-aligned too, and its kind is at the left.
    assert_eq!(text_x_of(&over, "offers"), text_x_of(&fits, "offers") - 10);
    assert_eq!(text_x_of(&over, "farm"), text_x_of(&fits, "farm"));
    // The tooltip of the wage follows the drawn wage: at the narrower edge it answers, at the old edge it does not.
    let field = |tip: &str, name: &str| {
        tip.split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{name}=")).map(str::to_owned))
            .unwrap_or_else(|| panic!("no {name} in {tip}"))
    };
    assert_eq!(
        field(&over_tip, "row").parse::<i64>().unwrap(),
        field(&over_tip, "full").parse::<i64>().unwrap() - 5
    );
    assert_eq!(field(&over_tip, "at_row_edge"), "wage", "{over_tip}");
    assert_eq!(field(&over_tip, "at_full_edge"), "nil", "{over_tip}");
    assert_eq!(field(&fit_tip, "at_full_edge"), "wage", "{fit_tip}");
}

#[test]
fn sorted_pop_row_opens_original_slot_and_return_preserves_the_list() {
    let mut rig = panel_rig(&[
        r"game.pop_members = function(place, slot, revision) note('opened_slot ' .. place .. ' ' .. slot .. ' ' .. revision); return {} end
time(1, 1); select_place('cards', 'Cards'); announce_tabs()",
        "announce_look()",
        "announce_panel_bar('cards')",
        "announce_ppicker()",
        r"note_kinds('by_people')
parent_stack = card_object; before_scroll = parent_stack:scroll_position()
parent_stack.items[2].on_click(); require('lib.pointer').dispatch({})",
        r"assert(window_objects.pop and not window_objects.pop.closed, 'pop detail opened')
window_objects.pop:close()
assert(not window_objects.Cards.closed, 'location remains open')
assert(parent_stack:scroll_position() == before_scroll, 'scroll restored')
note_kinds('after_close')",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    rig.opened();
    pclick(&mut rig, "pbar_sort");
    rig.opened();
    ppick_click(&mut rig, "people");
    rig.opened();
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded.iter().any(|line| line == "opened_slot cards 2 1"),
        "{recorded:?}"
    );
    assert_eq!(
        pnote(&rig, "by_people "),
        pnote(&rig, "after_close ").replace("after_close ", "by_people ")
    );
}

#[test]
fn sorted_filtered_building_opens_same_slot_by_row_and_card_and_escape_restores_list() {
    let mut rig = panel_rig(&[
        r"local first = VIEWS[1].cards.buildings[1]
first.stock = {{good='honey', amount=11, charged_ml=3850, outside=false}}
local second = {}; for k, v in pairs(first) do second[k] = v end
second.filled = 99; second.stock = {{good='honey', amount=37, charged_ml=12950, outside=false}}
VIEWS[1].cards.buildings[6] = second
colour_watch = true
time(1, 1); select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)
look_buttons.rows.on_click(); panel_bar:choose(2); panel_bar:toggle(1, 'farm')
assert(panel_shown[1].index == 6 and panel_shown[2].index == 1, 'sort and filter preserve original indices')
parent_stack = card_object; parent_bar = panel_bar; parent_scroll = parent_stack:scroll_position()
parent_stack.items[2].on_click(); require('lib.pointer').dispatch({})
assert(window_objects.building and not window_objects.building.closed, 'row opened building')
card_object:set_pos(10000); note('row_detail_done')",
        r"assert(not window_objects.Cards.closed, 'location stayed open')
assert(parent_bar.ticks == 1 and parent_bar.sort_index == 2, 'sort and filter stayed')
assert(parent_stack:scroll_position() == parent_scroll and look_buttons.rows.selected, 'look and scroll stayed')
look_buttons.cards.on_click(); assert(panel_shown[1].index == 6, 'card keeps original index')
parent_stack.items[1].on_click(); require('lib.pointer').dispatch({})
assert(window_objects.building and not window_objects.building.closed, 'card opened building')
card_object:set_pos(10000); note('card_detail_done')
window_objects.building:close()
assert(parent_bar.ticks == 1 and parent_bar.sort_index == 2, 'close preserves filter and sort')
assert(parent_stack:scroll_position() == parent_scroll and look_buttons.cards.selected, 'close preserves card look and scroll')
assert(tabs_by_title.Cards:current() == 'buildings', 'close preserves tab')",
    ]);
    rig.opened();
    assert!(rig.escape(), "Escape closes the detail panel");
    rig.opened();
    let seen = notes(&rig);
    let amount = seen
        .iter()
        .filter(|line| line.contains("#5fb35a=37"))
        .count();
    assert!(
        amount >= 2,
        "both row and card open the second farm's stock: {seen:?}"
    );
}

#[test]
fn removed_slot_closes_detail_and_click_queued_before_load_does_not_open() {
    let mut rig = rig(&[r"local first = VIEWS[1].cards.buildings[1]
local second = {}; for k, v in pairs(first) do second[k] = v end
second.stock = {{good='honey', amount=37, charged_ml=12950, outside=false}}
VIEWS[1].cards.buildings[6] = second
local later = {}; for k, v in pairs(VIEWS[1].cards) do later[k] = v end
later.buildings = {}; for i=1,5 do later.buildings[i] = VIEWS[1].cards.buildings[i] end
VIEWS[2].cards = later
time(1, 1); select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)
queued_click = card_object.items[6].on_click
queued_click(); require('lib.pointer').dispatch({})
assert(window_objects.building and not window_objects.building.closed, 'detail opened')
time(2, 2)
assert(window_objects.building.closed and not window_objects.Cards.closed, 'removed slot returns to location')
callbacks.time({year=768, month=1, day=1, hour=3, speed=1, economy_revision=2, events_revision=2, world=1})
queued_click(); require('lib.pointer').dispatch({})
assert(window_objects.building.closed, 'old-world queued click does nothing')
assert(not window_objects.Cards.closed, 'location stays open on load')
note('removed_and_queued_ok')"]);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "removed_and_queued_ok")
    );
}

#[test]
fn narrow_demesne_row_drops_fields_in_order_and_hits_only_visible_content() {
    let mut rig = rig_at(
        &[r"local b = VIEWS[1].cards.buildings[1]
b.kind='demesne'; b.filled=999; b.open=1; b.wage=1234567890
b.methods={{id='mixed_farming', levels=15}}
time(1, 1); select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)
look_buttons.rows.on_click()
local row = card_object.items[2]; local width = card_object.row_w // require('lib.ui').P
local wage = 'offers'; local count = '999/1000'; local kind = 'demesne'
local wage_left = width - 3 - font.layout(wage).width
local need = 16 + font.layout(kind).width + 13 + font.layout(count).width + 8
local positions = need <= wage_left
local method = (16 + font.layout(kind).width + 13 + (positions and font.layout(count).width + 4 or 0) + 4) <= wage_left
local textlib = require('lib.text'); local move = textlib.move; local drawn = {}
textlib.move = function(line, x, y) move(line, x, y); drawn[#drawn+1] = line.ck_test_text end
local made = row.draw(card_object.group, 0, 0, card_object.row_w); made:destroy(); textlib.move = move
local seen = {}; for x=0,width-1 do local _, part = row.tooltip(x, 2); if part then seen[part] = true end end
assert(seen.kind and seen.wage, 'kind and wage retain hits')
assert((seen.staff == true) == positions, 'positions hit agrees with the omission rule')
assert((seen.methods == true) == method, 'method hit agrees with the omission rule')
local joined = table.concat(drawn, '|')
assert(joined:find(wage, 1, true), 'the wage is drawn')
assert((joined:find(count, 1, true) ~= nil) == positions, 'positions drawing agrees with hits')
assert(joined:find(kind, 1, true) or joined:find('…', 1, true), 'kind is drawn or clipped')
note('narrow_row_ok')"],
        432,
        800,
        2,
    );
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "narrow_row_ok"));
}

#[test]
fn pop_portrait_opens_character_and_escape_returns_through_both_panels() {
    let mut rig = rig_with_prelude_at(
        characters::CHARACTERS,
        &[
            r"game.pop_members = function() return {CH[1][1]} end
time(1, 1); select_place('cards', 'Cards'); tabs_by_title.Cards:select('pops', true)
card_object.items[1].on_click(); require('lib.pointer').dispatch({})
assert(window_objects.pop and not window_objects.pop.closed, 'pop opened from card')
local stack = card_object; local item = stack.items[4]; local hit
for _, candidate in ipairs(item.hits) do if candidate.part == 'portrait' then hit = candidate; break end end
assert(hit and hit.open, 'portrait clickable')
local P = require('lib.ui').P
note(string.format('portrait_point %g %g', stack.origin_x + stack.x + (hit.x + 2) * P,
  stack.origin_y + stack.y + stack.tops[4] - stack:scroll_position() + (hit.y + 2) * P))",
            r"assert(window_objects.character and not window_objects.character.closed, 'character opened from portrait')
assert(not window_objects.pop.closed, 'pop remains below the character')",
            r"assert(window_objects.character.closed and not window_objects.pop.closed, 'closing character returns to pop')",
            r"assert(window_objects.pop.closed and not window_objects.Cards.closed, 'closing pop returns to location')",
        ],
        1800,
        900,
        2,
    );
    rig.opened();
    let point = ppoint(&rig, "portrait_point");
    pclick_at(&mut rig, point);
    rig.opened();
    assert!(rig.escape());
    rig.opened();
    assert!(rig.escape());
    rig.opened();
}

#[test]
fn escape_from_detail_preserves_a_scrolled_location_list() {
    let mut rig = rig(&[
        r"time(1, 1); select_place('scrolling', 'Scrolling'); tabs_by_title.Scrolling:select('pops', true)
look_buttons.rows.on_click(); parent_stack = card_object; parent_stack:set_pos(26)
assert(parent_stack:scroll_position() == 26, 'parent list actually scrolled')
parent_stack.items[3].on_click(); require('lib.pointer').dispatch({})
assert(window_objects.pop and not window_objects.pop.closed, 'pop opened')",
        r"assert(not window_objects.Scrolling.closed, 'location remains open')
assert(tabs_by_title.Scrolling:current() == 'pops', 'same tab')
assert(look_buttons.rows.selected, 'same look')
assert(parent_stack:scroll_position() == 26, 'same scroll offset')
note('scroll_restored')",
    ]);
    rig.opened();
    assert!(rig.escape());
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "scroll_restored"));
}
