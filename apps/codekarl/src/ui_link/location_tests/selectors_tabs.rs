use super::*;

#[test]
fn a_pops_selector_is_detached_before_a_resize_refit_and_keeps_look_and_scroll() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs()",
        "announce_look()",
        "card_object:set_pos(26); note('before_scroll ' .. card_object:scroll_position()); announce_look(); announce_selector_count()",
        "note('after_scroll ' .. card_object:scroll_position()); announce_look(); announce_selector_count()",
        "select_place('scrolling', 'Scrolling'); announce_tabs(); announce_selector_count()",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_look(&mut rig, "rows");
    rig.opened();
    rig.deliver_fixture_frame_at(1700, 800, 2);
    rig.opened();

    let recorded = notes(&rig);
    assert!(recorded.iter().any(|line| line == "before_scroll 26"));
    assert!(recorded.iter().any(|line| line == "after_scroll 26"));
    assert!(
        recorded
            .iter()
            .any(|line| line.starts_with("look rows ") && line.ends_with("selected=true"))
    );
    assert!(
        recorded
            .iter()
            .filter(|line| *line == "selector_count 1")
            .count()
            >= 2
    );

    assert!(rig.escape(), "Escape closes the refitted location panel");
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "selector_count 1"));
}

#[test]
fn a_location_tab_detaches_its_selector_before_resize_and_selection_change() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs()",
        "note('before_resize_selector_count ' .. attached_selector_count(panel_window))",
        "select_place('cards', 'Cards'); note('after_selection_selector_count ' .. attached_selector_count(panel_window))",
        "select_place('cards', 'Cards'); note('reopened_selector_count ' .. attached_selector_count(panel_window))",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    click_tab(&mut rig, "location");
    rig.opened();
    rig.deliver_fixture_frame_at(1700, 800, 2);
    rig.opened();

    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "before_resize_selector_count 0")
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "after_selection_selector_count 0")
    );
    assert!(rig.escape(), "Escape closes the location-only panel");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "reopened_selector_count 0")
    );
}

#[test]
fn switching_location_tabs_keeps_one_selector_and_close_reopen_starts_clean() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs(); note('selector_count ' .. attached_selector_count(panel_window))",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "announce_selector_count()",
        "select_place('scrolling', 'Scrolling'); announce_selector_count()",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_tab(&mut rig, "location");
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    click_tab(&mut rig, "buildings");
    rig.opened();
    click_tab(&mut rig, "location");
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();

    let counts = notes(&rig)
        .iter()
        .filter_map(|line| line.strip_prefix("selector_count "))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(counts, ["0", "1", "0", "1", "1", "0", "1"]);

    assert!(
        rig.escape(),
        "Escape closes the tab-switched location panel"
    );
    rig.opened();
    assert_eq!(
        notes(&rig)
            .iter()
            .filter_map(|line| line.strip_prefix("selector_count "))
            .next_back(),
        Some("1")
    );
}

#[test]
fn the_location_tab_stays_text_while_pops_and_buildings_start_as_cards_and_share_the_rows_choice() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling')",
        "announce_tabs()",
        "announce_look()",
        "note('theme track ' .. require('lib.theme').colour('track')); note('theme band ' .. require('lib.theme').colour('band'))",
        "card_object:set_pos(26); note('look_scroll_before ' .. card_object:scroll_position())",
        "note('look_scroll_after_switch ' .. card_object:scroll_position())",
        "select_place('nobody', 'No Economy'); announce_active_panel()",
        "announce_selector_liveness()",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    assert_eq!(
        card_stacks(&rig).last().unwrap(),
        &format!("gap=3 heights={}", vec!["61"; 60].join(","))
    );
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("look cards ") && line.ends_with("selected=true"))
    );
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "theme track #3a3326"));
    assert!(notes(&rig).iter().any(|line| line == "theme band #221c13"));
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "look_scroll_before 26")
    );
    click_look(&mut rig, "rows");
    assert_eq!(
        card_stacks(&rig).last().unwrap(),
        &format!("gap=1 heights={}", vec!["13"; 61].join(","))
    );
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "look_scroll_after_switch 0"),
        "switching looks resets scroll: {:?}",
        notes(&rig)
    );
    click_tab(&mut rig, "buildings");
    assert!(
        card_stacks(&rig)
            .last()
            .unwrap()
            .starts_with("gap=1 heights=")
    );
    rig.opened();
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13"),
        "another place keeps rows"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("active_panel ") && line.contains(" buildings "))
    );
    assert!(
        notes(&rig).iter().any(|line| line.ends_with("nobody@1")),
        "the new place was selected: {:?}",
        notes(&rig)
    );
    click_tab(&mut rig, "location");
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "selector_live false"),
        "location tab destroys selector buttons: {:?}",
        notes(&rig)
    );
    assert_eq!(
        rows(&rig).last().map(String::as_str),
        Some(
            "people: 0|  workers: 0|  dependents: 0|housing: basic 0/0, makeshift 0/0|crowding: 0%|food: 0 days|building land: 0 / 0 m²|storage: 0 / 0 L|lost to overflow last month: 0|good@16:left:#92ddea,price@212:right:#92ddea,base@238:right:#92ddea,buy/sell@246:left:#9a927e;stripe=-;icon=-"
        )
    );
}

#[test]
fn a_selection_reads_the_newest_revision_the_ui_has_seen() {
    let mut rig = rig(&[
        "time(1, 1) time(12, 2) select_place('rowan', 'Rowan')",
        "note('calls ' .. table.concat(place_calls, ','))",
    ]);
    rig.opened();
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("calls rowan@2")
    );
}

#[test]
fn a_selection_before_any_view_says_so_and_reads_nothing() {
    let mut rig = rig(&[
        "select_place('rowan', 'Rowan')",
        "note('calls ' .. #place_calls)",
        // The first view arrives: the open panel fills in.
        "time(1, 1)",
    ]);
    rig.opened();
    assert!(
        card_stacks(&rig).is_empty(),
        "a selection without a view creates no list yet"
    );
    rig.opened();
    assert_eq!(notes(&rig).last().map(String::as_str), Some("calls 0"));
    rig.opened();
    assert_eq!(rows(&rig).last().map(String::as_str), Some(LOCATION_ROWS));
    assert!(
        card_stacks(&rig).is_empty(),
        "the location tab stays a text list"
    );
}

#[test]
fn selecting_another_place_keeps_the_tab_showing() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan')",
        "announce_tabs()",
        "select_place('nobody', 'Nobody')",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    // The Pops tab is still showing for the other place.
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=13")
    );
}

// ---------------------------------------------------------------------------------------------
// The panel's list bars (CK-296, slice L3)
// ---------------------------------------------------------------------------------------------

#[test]
fn each_tab_has_its_bar_and_on_the_cards_tabs_it_shares_the_strip_with_the_selector_at_an_unchanged_height()
 {
    let mut rig = panel_rig(&[
        // Before any view: the panel says so, and the bar is drawn all the same.
        "select_place('cards', 'Cards') note('early ' .. tostring(panel_bar ~= nil)) announce_panel_bar('list')",
        "time(1, 1) announce_tabs()",
        "announce_look() announce_panel_bar('cards') note('cards_height ' .. card_object.height) note('selector ' .. look_buttons.cards.y .. ' ' .. look_buttons.rows.x)",
        "announce_look() announce_panel_bar('cards') note('cards_height ' .. card_object.height) note('selector ' .. look_buttons.cards.y .. ' ' .. look_buttons.rows.x)",
    ]);
    rig.opened();
    assert_eq!(pnote(&rig, "early "), "early true");
    let content = pnote(&rig, "pbar_content ");
    let numbers: Vec<f64> = content
        .split_whitespace()
        .skip(1)
        .map(|part| part.parse().unwrap())
        .collect();
    let (width, height) = (numbers[0], numbers[1]);
    assert_eq!(
        pnote(&rig, "pbar_sort_rect "),
        format!("pbar_sort_rect {} {} 26 30", width - 26.0, height - 34.0),
        "the location tab's bar under its list, with no view"
    );
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    for tab in ["pops", "buildings"] {
        let selector = pnote(&rig, "selector ");
        assert_eq!(
            selector,
            format!("selector {} 32", height - 34.0),
            "the selector's two buttons are where they are today on {tab}"
        );
        assert_eq!(
            pnote(&rig, "pbar_sort_rect "),
            format!("pbar_sort_rect {} {} 26 30", width - 26.0, height - 34.0),
            "the bar's buttons are in the same strip, at its right, on {tab}"
        );
        assert_eq!(
            pnote(&rig, "pbar_filter_rect "),
            format!("pbar_filter_rect {} {} 26 30", width - 58.0, height - 34.0)
        );
        assert_eq!(
            pnote(&rig, "cards_height "),
            format!("cards_height {}", height - 38.0),
            "the stack's height does not change on {tab}"
        );
        if tab == "pops" {
            click_tab(&mut rig, "buildings");
            rig.opened();
        }
    }
}

#[test]
fn a_tick_and_a_sort_stay_across_a_revision_a_window_size_the_same_place_and_the_look() {
    let mut rig = panel_rig(&[
        "VIEWS[1].rowan.prices[1].tags = {'food'} VIEWS[2].rowan.prices[1].tags = {'food'} time(1, 1) select_place('rowan', 'Rowan') panel_bar:toggle(1, 'food') panel_bar:choose(2) announce_panel_bar('list')",
        "time(12, 2) announce_panel_bar('list')",
        "announce_panel_bar('list')",
        "select_place('rowan', 'Rowan') announce_panel_bar('list') announce_tabs()",
        "panel_bar:toggle(1, 'peasant') panel_bar:choose(2) announce_panel_bar('cards') announce_look()",
        "time(13, 3) announce_panel_bar('cards')",
        "announce_panel_bar('cards') announce_look()",
        "announce_panel_bar('cards')",
    ]);
    let kept = |rig: &Rig, bars: u32| {
        let state = pnote(rig, "pstate ");
        assert!(
            state.starts_with(&format!("pstate ticks=1 sort=2 bars={bars} ")),
            "{state}"
        );
    };
    rig.opened();
    kept(&rig, 1);
    assert!(pnote(&rig, "pcount ").starts_with("pcount 1 of "));
    rig.opened();
    kept(&rig, 1);
    rig.deliver_fixture_frame_at(1500, 900, 2);
    rig.opened();
    kept(&rig, 1);
    rig.opened();
    kept(&rig, 1);
    click_tab_at(&mut rig, "pops", (1500, 900));
    rig.opened();
    kept(&rig, 2);
    rig.opened();
    kept(&rig, 2);
    rig.deliver_fixture_frame_at(1200, 800, 2);
    rig.opened();
    kept(&rig, 2);
    // The look changes from cards to rows: the bar and its choices are the same.
    click_look(&mut rig, "rows");
    rig.opened();
    kept(&rig, 2);
}

#[test]
fn the_choices_start_again_with_a_tab_another_place_a_ledger_pops_row_and_a_reopened_panel() {
    let mut rig = panel_rig(&[
        "time(1, 1) announce_bars() select_place('rowan', 'Rowan') panel_bar:choose(2) announce_panel_bar('list') announce_tabs()",
        "announce_panel_bar('cards')",
        "panel_bar:toggle(1, 'peasant') note('ticked ' .. panel_bar.ticks) select_place('cards', 'Cards') announce_panel_bar('cards') announce_tabs()",
        "announce_panel_bar('list') select_place('rowan', 'Rowan') panel_bar:choose(2) announce_panel_bar('list')",
        "announce_ledger()",
        "announce_panel_bar('cards')",
        "panel_bar:choose(2) window_objects.Rowan:close() select_place('rowan', 'Rowan') announce_panel_bar('cards')",
    ]);
    let state = |rig: &Rig, bars: u32, ticks: u32, sort: &str| {
        let line = pnote(rig, "pstate ");
        assert!(
            line.starts_with(&format!("pstate ticks={ticks} sort={sort} bars={bars} ")),
            "{line}"
        );
    };
    rig.opened();
    state(&rig, 1, 0, "2");
    // Another tab: a new bar with nothing chosen.
    click_tab(&mut rig, "pops");
    rig.opened();
    state(&rig, 2, 0, "nil");
    // Another place, on the same tab: the tick goes.
    rig.opened();
    assert_eq!(pnote(&rig, "ticked "), "ticked 1");
    state(&rig, 3, 0, "nil");
    // Back on the location tab (a new bar), then another place, then a choice: two bars made, one choice kept.
    click_tab(&mut rig, "location");
    rig.opened();
    state(&rig, 5, 0, "2");
    // A ledger pops row opens the pops tab of the same place: the tab changed, so the choices start again.
    press_bar(&mut rig, "pops");
    rig.opened();
    click_ledger_row(&mut rig, 1, 1800, 900);
    rig.opened();
    state(&rig, 6, 0, "nil");
    // The panel is closed and opened: a new bar.
    rig.opened();
    state(&rig, 7, 0, "nil");
}

#[test]
fn after_a_change_the_pops_stack_starts_at_its_top_and_the_location_list_keeps_its_scroll() {
    let mut rig = panel_rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling') announce_tabs()",
        "card_object:set_pos(200) note('before ' .. card_object:scroll_position()) panel_bar:toggle(1, 'peasant') note('after ' .. card_object:scroll_position()) panel_bar:choose(2) note('after_sort ' .. card_object:scroll_position())",
        "local goods = {} for index = 1, 30 do goods[index] = price(string.format('g%02d', index), 1000 + index, 10, 5, 4) end VIEWS[1].rowan.prices = goods select_place('rowan', 'Rowan') announce_tabs()",
        "list_object:set_pos(200) note('market_before ' .. list_object:scroll_position()) panel_bar:choose(2) note('market_after ' .. list_object:scroll_position()) panel_bar:choose(1) note('market_after_name ' .. list_object:scroll_position())",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    let number = |rig: &Rig, prefix: &str| -> i64 {
        pnote(rig, prefix)[prefix.len()..].trim().parse().unwrap()
    };
    assert!(number(&rig, "before ") > 0, "the stack was scrolled down");
    assert_eq!(
        number(&rig, "after "),
        0,
        "a tick puts the stack at its top"
    );
    assert_eq!(number(&rig, "after_sort "), 0, "and so does a sort choice");
    // The market, scrolled down: a sort choice leaves the list's scroll where it was.
    click_tab(&mut rig, "location");
    rig.opened();
    rig.opened();
    let before = number(&rig, "market_before ");
    assert!(before > 0, "the market list was scrolled down");
    assert_eq!(number(&rig, "market_after "), before);
    assert_eq!(number(&rig, "market_after_name "), before);
}
