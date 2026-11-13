use super::*;

/// The last announced centre of the mode button `key`, and whether it is selected.
fn mode_button(rig: &Rig, key: &str) -> (f64, f64, bool) {
    let prefix = format!("mode {key} ");
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no mode button {key}: {:?}", notes(rig)));
    let mut parts = line[prefix.len()..].split_whitespace();
    let x = parts.next().unwrap().parse().unwrap();
    let y = parts.next().unwrap().parse().unwrap();
    (x, y, parts.next().unwrap() == "true")
}

/// The modes the script has asked for, in order.
fn modes_called(rig: &Rig) -> Vec<String> {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("modes_called ").map(str::to_owned))
        .expect("modes were announced");
    line.split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Presses the mode button `key` and delivers a frame.
fn press_mode(rig: &mut Rig, key: &str) {
    let (x, y, _) = mode_button(rig, key);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

#[test]
fn the_mode_buttons_publish_the_mode_and_a_second_press_turns_it_off() {
    let mut rig = rig(&[
        "time(1, 1)",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
    ]);
    rig.opened();
    rig.opened();
    // Nothing is asked for, and nothing is selected, until a button is pressed.
    assert_eq!(modes_called(&rig), Vec::<String>::new());
    assert!(!mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2);
    press_mode(&mut rig, "stockpile");
    rig.opened();
    assert_eq!(modes_called(&rig), ["stockpile"]);
    assert!(
        mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2,
        "the mode that shows is selected"
    );
    // The same button again turns the mode off.
    press_mode(&mut rig, "stockpile");
    rig.opened();
    assert_eq!(modes_called(&rig), ["stockpile", "off"]);
    assert!(!mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2);
    // The other button changes the mode with no off between.
    press_mode(&mut rig, "yield");
    press_mode(&mut rig, "stockpile");
    rig.opened();
    assert_eq!(
        modes_called(&rig),
        ["stockpile", "off", "yield", "stockpile"]
    );
    assert!(mode_button(&rig, "stockpile").2 && !mode_button(&rig, "yield").2);
}

#[test]
fn the_elevation_button_publishes_the_elevation_mode_beside_the_other_two() {
    let mut rig = rig(&[
        "time(1, 1)",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
        "announce_modes()",
    ]);
    rig.opened();
    rig.opened();
    let selected =
        |rig: &Rig| ["stockpile", "yield", "elevation"].map(|key| mode_button(rig, key).2);
    assert_eq!(selected(&rig), [false; 3]);
    // The three buttons are in a row, left to right, on one line.
    let (stockpile, yields, elevation) = (
        mode_button(&rig, "stockpile"),
        mode_button(&rig, "yield"),
        mode_button(&rig, "elevation"),
    );
    assert!(stockpile.0 < yields.0 && yields.0 < elevation.0);
    assert!(
        (stockpile.1 - yields.1).abs() < 0.5 && (yields.1 - elevation.1).abs() < 0.5,
        "one line"
    );
    press_mode(&mut rig, "elevation");
    rig.opened();
    assert_eq!(modes_called(&rig), ["elevation"]);
    assert_eq!(selected(&rig), [false, false, true], "only elevation shows");
    // Another mode replaces it, with no off between; the elevation button again turns it off.
    press_mode(&mut rig, "yield");
    press_mode(&mut rig, "elevation");
    press_mode(&mut rig, "elevation");
    rig.opened();
    assert_eq!(
        modes_called(&rig),
        ["elevation", "yield", "elevation", "off"]
    );
    assert_eq!(selected(&rig), [false; 3]);
}

#[test]
fn a_mode_the_lane_refuses_leaves_the_buttons_and_the_mode_as_they_were() {
    let mut rig = rig(&[
        "time(1, 1)",
        "fail_mode = true announce_modes()",
        "announce_modes()",
        "fail_mode = false announce_modes()",
        "announce_modes()",
    ]);
    rig.opened();
    rig.opened();
    press_mode(&mut rig, "yield");
    rig.opened();
    // The refused press changed nothing: no mode was set, and no button is selected.
    assert_eq!(modes_called(&rig), Vec::<String>::new());
    assert!(!mode_button(&rig, "yield").2 && !mode_button(&rig, "stockpile").2);
    // Once the lane takes it, the same press works.
    rig.opened();
    press_mode(&mut rig, "yield");
    rig.opened();
    assert_eq!(modes_called(&rig), ["yield"]);
    assert!(mode_button(&rig, "yield").2);
}

#[test]
fn the_mode_bar_is_centred_at_the_bottom_of_the_window_and_follows_a_resize() {
    let mut rig = rig(&["time(1, 1) announce_bars()", "announce_bars()"]);
    rig.opened();
    // 1800 by 900 points. The bar is 110 by 38: centred it starts at (1800 - 110) / 2 = 845, which snaps to the 2-point grid as 846, and its
    // bottom edge is a margin of 8 above the window's: y = 900 - 38 - 8 = 854.
    assert_mode_bar_at(&rig, [846, 854]);
    let stockpile = bar_button(&rig, "stockpile");
    assert_eq!(stockpile[0], 846 + 5 * 2, "the first button's left edge");
    assert_eq!(
        stockpile[1] + stockpile[3] / 2,
        854 + 19,
        "the row's vertical centre"
    );
    let elevation = bar_button(&rig, "elevation");
    let left_gap = stockpile[0] - 5 * 2;
    let right_gap = 1800 - (elevation[0] + elevation[2] + 5 * 2);
    assert!(
        (left_gap - right_gap).abs() <= 2,
        "centred: {left_gap} points on the left, {right_gap} on the right"
    );
    // 1200 by 700: (1200 - 110) / 2 = 545, snapped 546; y = 700 - 38 - 8 = 654. The buttons moved with the bar.
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [546, 654]);
}

#[test]
fn the_closed_ledger_bar_has_two_icons_at_top_right_and_follows_a_resize() {
    let mut rig = rig(&["time(1, 1) announce_bars()", "announce_bars()"]);
    rig.opened();
    // The two 13 by 15 buttons sit 3 pixels apart inside the 39 by 19 bar, which is 8 points from the top and right edges.
    let check = |rig: &Rig, width: i64| {
        let locations = bar_button(rig, "location");
        let pops = bar_button(rig, "pops");
        assert_eq!(locations[1], 8 + 2 * 2, "the top of the locations button");
        assert_eq!([locations[2], locations[3]], [13 * 2, 15 * 2]);
        assert_eq!(pops[0], locations[0] + (13 + 3) * 2, "the 3-pixel gap");
        assert_eq!([pops[2], pops[3]], [13 * 2, 15 * 2]);
        assert!(
            (pops[0] + pops[2] + 5 * 2 + 8 - width).abs() <= 2,
            "the right edge of the ledger bar: {pops:?} in {width}"
        );
        assert!(
            bar_button(rig, "stockpile")[1] > locations[1] + locations[3],
            "the mode bar is below the ledger bar, not beside it"
        );
    };
    check(&rig, 1800);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "places_text_buttons 0"),
        "the old text button is retired"
    );
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    check(&rig, 1200);
}
