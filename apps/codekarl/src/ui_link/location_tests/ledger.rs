use super::*;
use ck_lua::HostFunction;
use ck_window::ScrollUnit;
use std::{collections::BTreeMap, sync::Arc};

fn scroll_lines(x: f64, y: f64, amount: i32) -> PointerEvent {
    PointerEvent::Scroll {
        position: points(x, y),
        amount: amount * 256,
        unit: ScrollUnit::Lines,
    }
}

fn ledger_content_width(rig: &Rig) -> f64 {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the ledger geometry");
    let content = line.split("content=").nth(1).expect("the content geometry");
    content.split_whitespace().next().unwrap().parse().unwrap()
}

fn ledger_close_point(rig: &Rig) -> (f64, f64) {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger_close "))
        .expect("the ledger close button");
    let mut values = line["ledger_close ".len()..].split_whitespace();
    (
        values.next().unwrap().parse().unwrap(),
        values.next().unwrap().parse().unwrap(),
    )
}

#[test]
fn the_ledger_opens_on_each_icon_docks_right_and_close_or_escape_restores_the_bar() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_tabs(); announce_ledger()",
        "announce_ledger()",
        "announce_ledger()",
    ]);
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("ledger 1200 8 600 892")
                && line.contains("draggable=false bar_visible=false tab=pops")),
        "{:?}",
        notes(&rig)
    );
    let pops = rows(&rig).last().unwrap().clone();
    assert!(pops.contains("Rowan@228:left:-"), "{pops}");
    let (x, y) = ledger_close_point(&rig);
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "ledger closed bar_visible=true"),
        "{:?}",
        notes(&rig)
    );

    press_bar(&mut rig, "location");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.contains("tab=locations")),
        "{:?}",
        notes(&rig)
    );
    assert!(rig.escape(), "Escape closes the topmost ledger");
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "ledger closed bar_visible=true"),
        "{:?}",
        notes(&rig)
    );
}

#[test]
fn ledger_rows_use_revision_order_colours_columns_unicode_clipping_and_select_the_place_tab() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_ledger()",
        "announce_active_panel()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let initial = rows(&rig).last().unwrap().clone();
    assert!(
        initial.contains("name@1:left:#92ddea,people@257:right:#92ddea,food@291:right:#92ddea"),
        "{initial}"
    );
    assert!(
        initial.contains("Rowan@1:left:-,120@257:right:-,215d@291:right:-"),
        "{initial}"
    );
    assert!(
        initial.contains("No Economy@1:left:-;stripe=-;icon=-"),
        "{initial}"
    );
    assert!(initial.contains("Cluain Mhic Nóis@1:left:-"), "{initial}");
    // At 1278 points wide, the right-third outer width is 426 points and its content is 207 UI pixels.
    rig.deliver_fixture_frame_at(1278, 900, 2);
    rig.opened();
    let narrow = rows(&rig).last().unwrap().clone();
    assert!(
        narrow.contains("people@170:right:#92ddea,food@204:right:#92ddea"),
        "{narrow}"
    );
    assert!(
        narrow.contains("\u{D3}\u{D3}\u{D3}..@1:left:-"),
        "the long name clips after whole code points: {narrow}"
    );
    assert!(
        narrow.contains("Cluain Mhic Nóis@1:left:-"),
        "the full Irish place name fits at the 207-unit content width: {narrow}"
    );

    // Select the visible Rowan row. Its immutable view is revision 1, and the ledger remains open beside the place panel.
    click_ledger_row(&mut rig, 2, 1278, 900);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "active_panel Rowan location ledger_closed=false"),
        "{:?}",
        notes(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("place_calls ") && line.contains("rowan@1")),
        "{:?}",
        notes(&rig)
    );
}

#[test]
fn selecting_a_ledger_place_renders_its_location_tab_from_the_game_place_view() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_ledger()",
        "note('location-opened')",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    // The fourth row is the two-pop monastery fixture (Cluain Mhic Nóis), after the heading,
    // Rowan and the empty place. This exercises the actual main.lua row-selection callback.
    click_ledger_row(&mut rig, 4, 1800, 900);
    rig.opened();
    let visible_rows = rows(&rig);
    assert!(
        visible_rows.iter().any(|line| line.starts_with("people:")),
        "the selected game's place view rendered its location tab: {visible_rows:?}"
    );
}

fn shipped_place_fixture() -> (HostFunction, String) {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let scenario_dir = assets.join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&scenario_dir).expect("the shipped Ireland scenario");
    let (state, _) = crate::game::economy::starting_economy(&assets, &scenario_dir, &scenario)
        .expect("the shipped starting economy");
    let rules = crate::game::economy::load_rules(&assets)
        .expect("the economy rules load")
        .expect("the shipped economy rules exist");
    let ids: Vec<String> = scenario
        .locations
        .iter()
        .map(|location| location.id.clone())
        .collect();
    let land = crate::game::economy::land_counts(&scenario, &rules.terrain_order)
        .expect("the shipped catchments load");
    let land_by_place = scenario
        .locations
        .iter()
        .zip(land)
        .map(|(location, counts)| (location.id.clone(), counts))
        .collect();
    let view = Arc::new(crate::game::view::build_with_land(
        Some(&rules),
        &state,
        &ids,
        &land_by_place,
        ck_sim::Date {
            year: 768,
            month: 5,
            day: 1,
            hour: 0,
        },
    ));
    let place_tables: BTreeMap<String, ConfigValue> = view
        .places
        .iter()
        .map(|place| (place.id.clone(), crate::game_api::place_table(place)))
        .collect();
    let actual_place: HostFunction = Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(id)),
            Some(ConfigValue::Integer(_revision)),
        ] = arguments
        else {
            return Err("game.place fixture expected id and revision".to_owned());
        };
        place_tables
            .get(id)
            .cloned()
            .map(|place| vec![place])
            .ok_or_else(|| format!("unexpected game.place id {id}"))
    });
    let escape_lua = |value: &str| value.replace('\\', "\\\\").replace('\'', "\\'");
    let place_rows = scenario
        .locations
        .iter()
        .map(|location| {
            let kind = &scenario.location_kinds.kinds[location.kind_index].id;
            format!(
                "{{id='{}', name='{}', kind='{}', size={}}}",
                escape_lua(&location.id),
                escape_lua(&location.name),
                escape_lua(kind),
                location.size
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    (actual_place, place_rows)
}

#[test]
fn selecting_a_shipped_ledger_place_renders_its_real_game_place_table() {
    let (actual_place, place_rows) = shipped_place_fixture();
    let setup = format!("place_data = {{{place_rows}}}; time(1, 1) announce_bars()");
    let observer = r"
local list = require('lib.list')
local real_list_new = list.new
list.new = function(spec)
  if spec.window.title ~= 'ledger' then
    local first = spec.items[1]
    if type(first) == 'table' then
      local cells = {}
      for _, cell in ipairs(first.cells) do cells[#cells + 1] = cell.text end
      first = table.concat(cells)
    end
    window.record('location tab ' .. first)
  end
  return real_list_new(spec)
end
";
    let script = script_with_prelude(
        observer,
        &[
            &setup,
            "announce_ledger()",
            "note('last place call ' .. place_calls[#place_calls])",
        ],
    );
    let mut rig =
        Rig::new_fixture_body_with_functions(&script, vec![("actual_place", actual_place)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let (origin_x, origin_y) = ledger_origin(&rig);
    rig.feed(&[scroll_lines(origin_x + 20.0, origin_y + 45.0, -1)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.feed(&[
        PointerEvent::Down(points(origin_x + 20.0, origin_y + 45.0)),
        PointerEvent::Up(points(origin_x + 20.0, origin_y + 45.0)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    let visible_rows = rows(&rig);
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "last place call clonmacnoise@1"),
        "the scrolled ledger click fetched Cluain Mhic Nóis from the shipped view: {:?}",
        notes(&rig)
    );
    assert!(
        visible_rows.iter().any(|line| line.starts_with("people:")),
        "a scrolled ledger row opens its shipped game.place view: {visible_rows:?}"
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("location tab people:")),
        "the window witness can log the first typed location row: {:?}",
        notes(&rig)
    );
}

#[test]
fn real_ledger_hover_uses_the_drawn_row_cell_after_scroll_and_resize() {
    let mut rig = rig(&[
        "use_many_ledger_places(); time(1, 1) announce_bars()",
        "announce_ledger()",
        "announce_ledger_cell_tips()",
        "announce_ledger()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    rig.opened();
    assert_ledger_heading_and_place_tooltips(&rig);
    let (origin_x, origin_y) = ledger_origin(&rig);
    let width = ledger_content_width(&rig);
    let people_point = (origin_x + width - 84.0, origin_y + 39.0);
    let row_key = rig
        .key_at(people_point.0, people_point.1)
        .expect("the second ledger row is hit");
    assert!(row_key.ends_with("_row_2"), "{row_key}");
    rig.feed(&[PointerEvent::Move(points(people_point.0, people_point.1))]);
    for _ in 0..35 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("source ")
            && line.contains("peasants 120|workers 80, dependents 40 part=2")),
        "the real pointer and tooltip update reach this row's people source: {:?}",
        notes(&rig)
    );

    hover_food_cell_after_delay(&mut rig, (origin_x + width - 20.0, origin_y + 39.0));

    rig.feed(&[scroll_lines(people_point.0, people_point.1, -1)]);
    // Let the changed scroll position publish its new row hit regions before asking the same screen point again.
    rig.deliver_fixture_frame_at(1800, 900, 2);
    // Scrolling moves the rows beneath a stationary pointer. Publish the same pointer position again so the real
    // router asks the row that is now under it, rather than retaining the pre-scroll hover target.
    // A one-point nudge and return produces distinct cursor events while ending at the exact original screen point.
    rig.feed(&[PointerEvent::Move(points(
        people_point.0 + 1.0,
        people_point.1,
    ))]);
    rig.feed(&[PointerEvent::Move(points(people_point.0, people_point.1))]);
    for _ in 0..35 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("source ")
            && line.contains("peasants 180|monks 5|workers 108, dependents 72 part=2")),
        "the same pointer position follows the scrolled row to Cards: {:?}",
        notes(&rig)
    );

    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    let (resized_x, resized_y) = ledger_origin(&rig);
    let resized_width = ledger_content_width(&rig);
    let resized_point = (resized_x + resized_width - 84.0, resized_y + 39.0);
    rig.feed(&[PointerEvent::Move(points(resized_point.0, resized_point.1))]);
    for _ in 0..35 {
        rig.deliver_fixture_frame_at(1200, 700, 2);
    }
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("source ")
            && line.contains("peasants 120|workers 80, dependents 40 part=2")),
        "the moved ledger routes its resized people cell: {:?}",
        notes(&rig)
    );
}

fn assert_ledger_heading_and_place_tooltips(rig: &Rig) {
    let captured = notes(rig);
    for name in [
        "heading_name",
        "heading_people",
        "heading_food",
        "place_name",
        "place_people",
        "place_food",
    ] {
        assert!(
            captured
                .iter()
                .any(|line| line == &format!("tooltip_kind {name} lockable")),
            "ledger {name} remains lockable: {captured:?}"
        );
    }
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip heading_food title=food days|")),
        "the ledger food heading opens the food-days explanation: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip place_name title=Rowan|")),
        "the place-name cell opens that place's location content: {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("tooltip place_food peasant Ulaid 215 days part=3")),
        "the place food cell opens the food breakdown with cell part 3: {captured:?}"
    );
}

fn hover_food_cell_after_delay(rig: &mut Rig, point: (f64, f64)) {
    let before = notes(rig);
    let people_tooltip_panels = before
        .iter()
        .filter(|line| line.starts_with("tooltip_panel_quad "))
        .count();
    assert!(
        people_tooltip_panels > 0,
        "the people tooltip was drawn: {before:?}"
    );
    rig.feed(&[PointerEvent::Move(points(point.0, point.1))]);
    for _ in 0..10 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    let waiting = notes(rig);
    assert_eq!(
        waiting
            .iter()
            .filter(|line| line.starts_with("tooltip_panel_quad "))
            .count(),
        people_tooltip_panels,
        "moving from people to food does not draw a new tooltip before the restarted delay"
    );
    for _ in 0..25 {
        rig.deliver_fixture_frame_at(1800, 900, 2);
    }
    let opened = notes(rig);
    assert!(
        opened
            .iter()
            .any(|line| line.starts_with("source ")
                && line.contains("peasant Ulaid 215 days part=3")),
        "after the restarted delay, the real food-cell hover opens place_food: {opened:?}"
    );
    assert!(
        opened
            .iter()
            .filter(|line| line.starts_with("tooltip_panel_quad "))
            .count()
            > people_tooltip_panels,
        "the food tooltip is drawn after the restarted delay"
    );
}

#[test]
fn refreshed_ledger_rows_keep_their_place_mapping_after_inserts_shifts_and_removals() {
    let mut rig = rig(&[
        "time(0, 1) announce_bars()",
        "place_data = {{id='before', name='Aardvark', kind='town', size=1}, \
         {id='rowan', name='Rowan', kind='town', size=3}, \
         {id='fada', name='Cluain Mhic Nóis', kind='monastery', size=2}, \
         {id='long', name=string.rep('Ó', 40), kind='town', size=1}}; time(1, 2); announce_ledger()",
        "place_calls = {}",
        "announce_last_place_call()",
        "place_calls = {}",
        "announce_last_place_call()",
        "place_calls = {}",
        "announce_last_place_call()",
        "place_data = {{id='rowan', name='Rowan', kind='town', size=3}, \
         {id='long', name='Ó', kind='town', size=1}}; time(2, 1); announce_ledger()",
        "place_calls = {}",
        "announce_last_place_call()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    assert!(rows(&rig).last().unwrap().contains("Aardvark@1:left"));

    // The inserted first place, the shifted second place, and the last place must use this revision's row map.
    rig.opened();
    click_ledger_row(&mut rig, 2, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call before@2")
    );
    rig.opened();
    click_ledger_row(&mut rig, 3, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call rowan@2")
    );
    rig.opened();
    click_ledger_row(&mut rig, 5, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call long@2")
    );

    // Removing rows also replaces their mapping; the new last row is long, not the stale fada entry.
    rig.opened();
    rig.opened();
    click_ledger_row(&mut rig, 3, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call long@1")
    );
}

#[test]
fn a_ledger_filled_after_an_empty_first_revision_gets_a_current_row_mapping() {
    let mut rig = rig(&[
        "place_data = {}; time(0, 1) announce_bars()",
        "place_data = {{id='before', name='Aardvark', kind='town', size=1}}; time(1, 2); announce_ledger()",
        "place_calls = {}",
        "announce_last_place_call()",
    ]);
    rig.opened();
    press_bar(&mut rig, "location");
    assert_eq!(
        rows(&rig).last().unwrap().split('|').count(),
        1,
        "only the header is present initially"
    );
    assert!(rows(&rig).last().unwrap().starts_with("name@"));

    rig.opened();
    rig.opened();
    click_ledger_row(&mut rig, 2, 1800, 900);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("last_place_call before@2")
    );
}

#[test]
fn a_pops_ledger_row_selects_pops_and_updates_the_open_location_tab() {
    for previous_tab in ["location", "buildings"] {
        let mut rig = rig(&[
            "time(1, 1) select_place('rowan', 'Rowan') announce_bars(); announce_place_tabs()",
            "announce_place_tabs()",
            "announce_ledger_tabs(); announce_ledger()",
            "announce_place_tabs()",
            "announce_place_tabs()",
            "select_place('fada', 'Fada'); announce_place_tabs()",
        ]);
        rig.opened();
        if previous_tab == "buildings" {
            click_tab(&mut rig, "buildings");
        }
        rig.opened();

        press_bar(&mut rig, "location");
        press_bar(&mut rig, "pops");
        rig.opened();
        click_ledger_row(&mut rig, 1, 1800, 900);
        rig.opened();
        let current = notes(&rig);
        assert!(
            current.iter().any(|line| line == "place_selected pops"),
            "{current:?}"
        );
        assert!(
            current.iter().any(|line| line == "place_cards_open true"),
            "{current:?}"
        );
        assert!(
            current.iter().any(|line| line.contains(" peasant")),
            "Pops content was not drawn: {current:?}"
        );
        assert!(
            current
                .iter()
                .any(|line| line.starts_with("tab pops ") && line.ends_with("selected=true")),
            "Pops tab is not selected: {current:?}"
        );

        click_tab(&mut rig, previous_tab);
        rig.opened();
        let current = notes(&rig);
        assert!(
            current
                .iter()
                .any(|line| line == &format!("place_selected {previous_tab}")),
            "{current:?}"
        );
        if previous_tab == "location" {
            assert!(
                current.iter().any(|line| line == "place_cards_open false"),
                "{current:?}"
            );
        } else {
            assert!(
                current.iter().any(|line| line == "place_cards_open true"),
                "{current:?}"
            );
        }

        rig.opened();
        let current = notes(&rig);
        assert!(
            current
                .iter()
                .any(|line| line == &format!("place_selected {previous_tab}")),
            "show without tab changed it: {current:?}"
        );
    }
}

#[test]
fn the_pops_ledger_clips_inside_its_column_and_tracks_the_1200_by_700_resize() {
    let mut rig = rig(&[
        "time(1, 1) announce_bars()",
        "announce_pop_clip(); announce_ledger()",
        "announce_pop_clip(); announce_ledger()",
    ]);
    rig.opened();
    press_bar(&mut rig, "pops");

    // A 1278-point view makes the right-third ledger 426 points wide and its clipped row column 87 UI pixels wide.
    rig.deliver_fixture_frame_at(1278, 900, 2);
    rig.opened();
    let clip = notes(&rig)
        .into_iter()
        .find(|line| line.starts_with("pop_clip "))
        .expect("the long pop label's rendered extent");
    let field = |name: &str| -> u32 {
        clip.split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("missing {name}: {clip}"))
            .parse()
            .unwrap_or_else(|_| panic!("bad {name}: {clip}"))
    };
    assert_eq!(
        field("x"),
        16,
        "the label starts at its specified column: {clip}"
    );
    assert!(
        field("right") < 103,
        "the clipped glyphs end before x=103: {clip}"
    );

    // At 1200x700 points the ledger remains the right third: 400 points wide after the resize.
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    let ledger = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the resized ledger geometry");
    let width = ledger
        .split_whitespace()
        .nth(3)
        .expect("ledger width")
        .parse::<u32>()
        .expect("numeric ledger width");
    assert_eq!(
        width, 400,
        "the ledger follows the resized right third: {ledger}"
    );
}
