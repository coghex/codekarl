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
