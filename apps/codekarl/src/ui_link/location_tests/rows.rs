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
