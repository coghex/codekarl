use super::*;

fn click_panel_point(rig: &mut Rig, x: f64, y: f64) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
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
fn row_items_expand_exclusively_keep_open_rows_across_revisions_and_scroll_clamps() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "announce_look()",
        "time(12, 2)",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,13")
    );
    // The content origin is x=6, y=82 points. The first data row starts 14 UI pixels below it.
    click_panel_point(&mut rig, 40.0, 123.0);
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,100,13")
    );
    assert!(
        notes(&rig).iter().any(|line| line.contains("#4fb3c8")),
        "the open row uses the place outline band: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("#15110a peasant ulaid")),
        "the open row uses title-text ink: {:?}",
        notes(&rig)
    );
    let recorded = notes(&rig);
    assert!(
        card_run(&recorded, "#663069", "180"),
        "open pop header digits: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#15110a", "+"),
        "open pop header sign: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#92ddea", "ulaid"),
        "open pop culture: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#5fb35a", "108"),
        "open pop detail numbers: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#ece3cb", " · "),
        "open pop row separator keeps text colour: {recorded:?}"
    );
    // Once the first row expands, the second row begins at 115 UI pixels below the content origin.
    click_panel_point(&mut rig, 40.0, 325.0);
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,100")
    );
    rig.opened();
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,100")
    );
    assert!(
        rig.sink.quads().iter().any(|quad| quad.clip.is_some()),
        "rows are clipped by their content viewport"
    );
}

#[test]
fn an_open_storehouse_row_keeps_its_plain_storage_fallback() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "tabs_by_title.Cards:select('buildings', true); look_buttons.rows.on_click(); card_object.items[4].on_click()",
    ]);
    rig.opened();
    rig.opened();
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        card_run(&recorded, "#ece3cb", "storage"),
        "the opened storehouse row retains its plain storage fallback: {recorded:?}"
    );
    assert!(
        card_run_at(&recorded, 32, 110, "#ece3cb", "storage"),
        "the storehouse method fallback stays plain text: {recorded:?}"
    );
    assert!(
        card_run_at(&recorded, 32, 134, "#92ddea", "storage"),
        "the open-row storage label is the glossary term: {recorded:?}"
    );
}

#[test]
fn an_open_building_row_keeps_the_inverted_header_number_tints() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
        "announce_look()",
        "time(12, 2)",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "buildings");
    rig.opened();
    click_look(&mut rig, "rows");
    click_panel_point(&mut rig, 40.0, 123.0);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded.iter().any(|line| line.contains("#4fb3c8")),
        "the open row keeps its cyan band: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#15110a", "farm"),
        "open header words stay dark: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#663069", "80"),
        "open header digits use highlight_number: {recorded:?}"
    );
    assert!(
        card_run(&recorded, "#15110a", "/"),
        "open header punctuation stays dark: {recorded:?}"
    );
}

#[test]
fn opening_and_closing_a_row_keeps_the_scroll_offset() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling')",
        "announce_tabs()",
        "announce_look()",
        "card_object:set_pos(26); note('row_scroll_before ' .. card_object:scroll_position())",
        "card_object.items[2].on_click(); note('row_scroll_after_open ' .. card_object:scroll_position())",
        "card_object.items[2].on_click(); note('row_scroll_after_close ' .. card_object:scroll_position())",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_scroll_before 26")
    );

    // Invoke the same row callback as a hit after scrolling one line; pointer dispatch is covered above.
    rig.opened();
    assert!(
        card_stacks(&rig)
            .last()
            .unwrap()
            .starts_with("gap=1 heights=13,100,"),
        "first row expanded: {:?}",
        card_stacks(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_scroll_after_open 26"),
        "opening retains the offset: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("cardquad ") && line.ends_with("#4fb3c8")),
        "open row uses place outline: {:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("#15110a peasant ulaid")),
        "open row title uses title-text: {:?}",
        notes(&rig)
    );
    assert!(
        rig.sink.quads().iter().any(|quad| quad.clip.is_some()),
        "the expanded rows remain clipped to the content viewport"
    );

    // The second click invokes the refreshed row callback and closes it again.
    rig.opened();
    assert!(
        card_stacks(&rig)
            .last()
            .unwrap()
            .starts_with("gap=1 heights=13,13,13,"),
        "second click closes the row: {:?}",
        card_stacks(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "row_scroll_after_close 26"),
        "closing retains the offset: {:?}",
        notes(&rig)
    );
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

#[test]
fn pop_rows_by_people_open_and_hover_the_pop_at_its_original_index_and_an_open_row_a_tick_hides_opens_again()
 {
    let mut rig = panel_rig(&[
        "time(1, 1) select_place('cards', 'Cards') announce_tabs()",
        "announce_look()",
        "announce_panel_bar('cards')",
        "announce_ppicker()",
        "note_kinds('by_people') note(string.format('first_row %g %g', card_object.origin_x + 40, card_object.origin_y + 14 * 2 + 6))",
        r"local P = require('lib.ui').P
card_object.items[2].tooltip(card_object.width // 2 - 75, 2)
note('pop_hover ' .. last_content.class .. ' ' .. last_content.args.place .. ' ' .. last_content.args.pop)",
        "announce_ppicker()",
        "note_kinds('hidden')",
        "note_kinds('back')",
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
    assert_eq!(
        pnote(&rig, "by_people "),
        "by_people peasant@3,peasant@1,noble@4,monk@2,monk@5",
        "most people first, the third pop at the top"
    );
    // A click on the first row opens that pop's row.
    let first = ppoint(&rig, "first_row");
    pclick_at(&mut rig, first);
    let opened = card_stacks(&rig).last().cloned().expect("a stack");
    assert!(
        opened.starts_with("gap=1 heights=13,")
            && !opened.starts_with("gap=1 heights=13,13,13,13,13,13"),
        "the first row is open: {opened}"
    );
    rig.opened();
    assert_eq!(pnote(&rig, "pop_hover "), "pop_hover pop_size cards 3");
    // A tick hides the open row; removing the tick shows it open again.
    pclick(&mut rig, "pbar_filter");
    rig.opened();
    ppick_click(&mut rig, "monk");
    rig.opened();
    assert_eq!(pnote(&rig, "hidden "), "hidden monk@2,monk@5");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,13")
    );
    ppick_click(&mut rig, "monk");
    rig.opened();
    assert_eq!(
        pnote(&rig, "back "),
        "back peasant@3,peasant@1,noble@4,monk@2,monk@5"
    );
    assert_eq!(
        card_stacks(&rig).last(),
        Some(&opened),
        "the open row is open again"
    );
}

#[test]
fn rows_taller_than_their_view_are_drawn_5_pixels_narrower_and_opening_a_row_or_a_filter_crosses_the_boundary_both_ways()
 {
    let mut rig = rig_with_prelude_at(
        &format!("{PANEL_BARS}\n{PANEL_FIXTURE}\n{}", cards::CARD_WIDTH_SPY),
        &[
            "time(1, 1) select_place('cards', 'Cards') announce_tabs()",
            "announce_tabs(); tabs_by_title.Cards:select('pops', true)",
            "card_widths = {}; look_buttons.rows.on_click()",
            // The header and five closed rows fit the 202 points of view.
            r"expect_that(card_object.content_height <= card_object.height, 'closed rows fit')
local width = panel_window.content_width
eq_that(card_object.row_w, width, 'the rows are the view wide')
for _, w in ipairs(card_widths) do eq_that(w, width, 'every row and the header drawn at the full width') end
card_widths = {}
card_object.items[2].on_click()",
            // Opening a row makes the stack overflow: every item is drawn again, 5 pixels narrower.
            r"expect_that(card_object.content_height > card_object.height, 'an open row overflows')
local width = panel_window.content_width
eq_that(card_object.row_w, width - 10, 'the stack gives the row width')
expect_that(#card_widths > 0, 'the items in view were drawn again')
for _, w in ipairs(card_widths) do eq_that(w, width - 10, 'the header and every row drawn again, narrower') end
panel_bar:toggle(1, 'noble')",
            // A filter that hides the open row makes it fit again.
            r"local width = panel_window.content_width
expect_that(card_object.content_height <= card_object.height, 'a filtered stack fits')
eq_that(card_widths[#card_widths], width, 'drawn at the full width again')
panel_bar:clear()",
            r"local width = panel_window.content_width
eq_that(card_widths[#card_widths], width - 10, 'the open row is back, and the narrower width with it')",
        ],
        1800,
        340,
        2,
    );
    for _ in 0..7 {
        rig.opened();
    }
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
    // The header: `building` is at the left and `positions` and `wage` are right-aligned.
    assert_eq!(text_x_of(&over, "building"), text_x_of(&fits, "building"));
    assert_eq!(
        text_x_of(&over, "positions"),
        text_x_of(&fits, "positions") - 10
    );
    assert_eq!(text_x_of(&over, "wage"), text_x_of(&fits, "wage") - 10);
    // The first building's row: its wage is right-aligned too, and its kind is at the left.
    assert_eq!(text_x_of(&over, "3.2"), text_x_of(&fits, "3.2") - 10);
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
