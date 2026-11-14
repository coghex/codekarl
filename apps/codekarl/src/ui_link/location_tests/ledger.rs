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

pub(super) fn shipped_place_fixture() -> (HostFunction, String) {
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

// ---------------------------------------------------------------------------------------------
// The ledger's list bars (CK-295, slice L2)
// ---------------------------------------------------------------------------------------------

/// Observers for the list bar under the ledger's list: the bar the shipped script made, the rows it last gave the list (and the rows
/// it was given), the content class the last tooltip asked for, and where the picker's rows are.
const BAR_OBSERVERS: &str = r#"
local listbar_library = require("lib.listbar")
local real_listbar_new = listbar_library.new
bars_created = 0
listbar_library.new = function(spec)
  local created = real_listbar_new(spec)
  -- Only the ledger's bar: the location panel has bars of its own (CK-296).
  if spec.window.title ~= "ledger" then return created end
  bars_created = bars_created + 1
  bar_object = created
  local real_view = created.view
  function created:view(rows)
    local shown = real_view(self, rows)
    last_shown, last_given = shown, rows
    return shown
  end
  return created
end
local tooltip_library = require("lib.tooltip")
local real_tooltip_content = tooltip_library.content
tooltip_library.content = function(class, args)
  last_content = {class = class, args = args}
  return real_tooltip_content(class, args)
end
spy_hits, spy_offsets = {}, {}
local real_spy_hit = render.set_hit
render.set_hit = function(key, rects, group)
  spy_hits[key] = rects ~= nil and {rects = rects, group = group} or nil
  return real_spy_hit(key, rects, group)
end
local real_spy_offset = render.set_group_offset
render.set_group_offset = function(group, x, y)
  spy_offsets[group] = {x, y}
  return real_spy_offset(group, x, y)
end
function hit_centre(key)
  local kept = spy_hits[key]
  local offset = spy_offsets[kept.group] or {0, 0}
  local rect = kept.rects[1]
  return rect.x + offset[1] + rect.width / 2, rect.y + offset[2] + rect.height / 2
end
function announce_bar()
  local ox, oy = window_objects.ledger:content_origin()
  for name, control in pairs({filter = bar_object.filter_button, sort = bar_object.sort_button}) do
    local width, height = control:size()
    note(string.format("bar_%s %g %g", name, ox + control.x + width / 2, oy + control.y + height / 2))
    note(string.format("bar_%s_rect %g %g %g %g", name, control.x, control.y, width, height))
  end
  note(string.format("ledger_list rows=%d view=%d content=%d", list_object.rows, list_object.view_height,
    window_objects.ledger.content_height))
  note("bar_state ticks=" .. bar_object.ticks .. " sort=" .. tostring(bar_object.sort_index) .. " bars=" .. bars_created)
  note("count " .. tostring(bar_object.count.ck_test_text or ""))
  note("shown " .. #last_shown .. " of " .. #last_given .. " scroll=" .. list_object:scroll_position())
  local ids = {}
  for index, item in ipairs(last_shown) do ids[index] = item.place.id end
  note("shown_ids " .. table.concat(ids, ","))
end
function announce_picker()
  local picker = bar_object.picker
  local labels = {}
  for index, row in ipairs(picker.rows) do
    labels[index] = row.entry.label
    if row.key ~= nil then
      local x, y = hit_centre(row.key)
      note(string.format("pick|%s|%g|%g", row.entry.label, x, y))
    end
  end
  note("picker " .. picker.kind .. " " .. table.concat(labels, ","))
end
-- The people column of the list in order: the ones with a number must come first, in the given direction, then the ones without.
function check_people_order(descending)
  local items = list_object.items
  local without, with, previous = 0, 0, nil
  for index = 2, #items do
    local cell = items[index].cells[2]
    if cell == nil then
      without = without + 1
    else
      expect_that(without == 0, "a place without people is before one with")
      local value = tonumber(cell.text)
      if previous ~= nil then
        expect_that(descending and previous >= value or not descending and previous <= value, "people out of order at " .. index)
      end
      previous = value
      with = with + 1
    end
  end
  note("people_order with=" .. with .. " without=" .. without .. " first=" .. tostring(previous))
end
function expect_that(ok, message) if not ok then error(message, 2) end end
function eq_that(actual, wanted, message)
  if actual ~= wanted then error(message .. ": got " .. tostring(actual) .. ", wanted " .. tostring(wanted), 2) end
end
"#;

/// A rig on the shipped Ireland places (94 of them) with the ledger open on `tab` (`location` or `pops`); `steps` run, one per
/// `Opened`, after the ledger opened.
fn shipped_ledger_rig(tab: &str, steps: &[&str]) -> Rig {
    shipped_ledger_rig_with(tab, "", steps)
}

/// [`shipped_ledger_rig`] with `extra` Lua run before the shipped script.
fn shipped_ledger_rig_with(tab: &str, extra: &str, steps: &[&str]) -> Rig {
    let (actual_place, place_rows) = shipped_place_fixture();
    let setup = format!("place_data = {{{place_rows}}}; time(1, 1) announce_bars()");
    let mut all: Vec<&str> = vec![&setup];
    all.extend(steps);
    let prelude = format!("{BAR_OBSERVERS}\n{extra}");
    let script = script_with_prelude(&prelude, &all);
    let mut rig =
        Rig::new_fixture_body_with_functions(&script, vec![("actual_place", actual_place)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    press_bar(&mut rig, tab);
    rig
}

/// The ids and names of the shipped places of one kind, in the scenario file's order.
fn shipped_places_of(kind: &str) -> Vec<(String, String)> {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let scenario = ck_map::load_scenario(assets.join("scenarios/ireland-768"))
        .expect("the shipped Ireland scenario");
    scenario
        .locations
        .iter()
        .filter(|location| scenario.location_kinds.kinds[location.kind_index].id == kind)
        .map(|location| (location.id.clone(), location.name.clone()))
        .collect()
}

fn note_with(rig: &Rig, prefix: &str) -> String {
    notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no note {prefix}: {:?}", notes(rig)))
}

fn note_point(rig: &Rig, name: &str) -> (f64, f64) {
    let line = note_with(rig, &format!("{name} "));
    let mut parts = line[name.len() + 1..].split_whitespace();
    (
        parts.next().unwrap().parse().unwrap(),
        parts.next().unwrap().parse().unwrap(),
    )
}

/// The centre of the picker row labelled `label`, as the last `announce_picker` marked it.
fn pick_point(rig: &Rig, label: &str) -> (f64, f64) {
    let prefix = format!("pick|{label}|");
    let line = note_with(rig, &prefix);
    let mut parts = line[prefix.len()..].split('|');
    (
        parts.next().unwrap().parse().unwrap(),
        parts.next().unwrap().parse().unwrap(),
    )
}

fn click_point(rig: &mut Rig, (x, y): (f64, f64)) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

#[test]
fn the_places_list_is_shorter_by_the_bar_and_its_pickers_offer_the_kinds_and_the_three_sorts() {
    let mut rig = shipped_ledger_rig(
        "location",
        &[
            "announce_ledger() announce_bar()",
            "announce_picker()",
            "announce_picker()",
        ],
    );
    rig.opened();
    let geometry = note_with(&rig, "ledger_list ");
    let numbers: Vec<u32> = geometry
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap())
        .collect();
    let (list_rows, view, content) = (numbers[0], numbers[1], numbers[2]);
    // The content is 892 - title - padding tall; the bar takes 19 UI pixels (38 points) at its bottom, and the list is whole rows.
    assert_eq!(view, list_rows * 26, "{geometry}");
    assert_eq!(list_rows, (content - 38) / 26, "{geometry}");
    // Its buttons: 13 by 15 UI pixels, their tops 17 pixels above the content's bottom edge, the sort at the content's right edge.
    let filter = note_with(&rig, "bar_filter_rect ");
    let sort = note_with(&rig, "bar_sort_rect ");
    let width = ledger_content_width(&rig);
    assert_eq!(
        sort,
        format!(
            "bar_sort_rect {} {} 26 30",
            width - 26.0,
            f64::from(content) - 34.0
        )
    );
    assert_eq!(
        filter,
        format!(
            "bar_filter_rect {} {} 26 30",
            width - 58.0,
            f64::from(content) - 34.0
        )
    );
    assert!(
        f64::from(view) <= f64::from(content) - 34.0,
        "the list ends above the buttons"
    );

    click(&mut rig, "bar_filter");
    rig.opened();
    assert_eq!(
        note_with(&rig, "picker "),
        "picker filter crannog,monastery,ringfort,royal site"
    );
    click(&mut rig, "bar_sort");
    rig.opened();
    assert_eq!(
        note_with(&rig, "picker "),
        "picker sort default,name,people,food"
    );
}

fn click_pick(rig: &mut Rig, label: &str) {
    let at = pick_point(rig, label);
    click_point(rig, at);
}

fn click(rig: &mut Rig, name: &str) {
    let at = note_point(rig, name);
    click_point(rig, at);
}

#[test]
fn ticking_ringfort_shows_the_header_and_the_ten_ringforts_in_file_order_and_their_rows_still_click_and_hover_as_places()
 {
    let ringforts = shipped_places_of("ringfort");
    assert_eq!(ringforts.len(), 10, "the shipped scenario's ringforts");
    let mut rig = shipped_ledger_rig(
        "location",
        &[
            "announce_ledger() announce_bar()",
            "announce_picker()",
            // The third shown row's name cell: the hover source gives that place's location tooltip.
            "announce_bar() note('header ' .. list_object.items[1].cells[1].text .. ' items=' .. #list_object.items) list_object.items[4].tooltip(0, 0, 1) note('hover_class ' .. last_content.class .. ' ' .. tostring(last_content.args.place))",
            "announce_last_place_call()",
        ],
    );
    rig.opened();
    assert!(
        note_with(&rig, "shown ").starts_with("shown 94 of 94"),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(
        note_with(&rig, "count "),
        "count ",
        "no count with nothing ticked"
    );
    click(&mut rig, "bar_filter");
    rig.opened();
    click_pick(&mut rig, "ringfort");
    rig.opened();
    let expected_ids: Vec<&str> = ringforts.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        note_with(&rig, "shown_ids "),
        format!("shown_ids {}", expected_ids.join(","))
    );
    assert!(
        note_with(&rig, "shown ").starts_with("shown 10 of 94 scroll=0"),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(note_with(&rig, "count "), "count 10 of 94");
    assert_eq!(note_with(&rig, "header "), "header name items=11");
    assert_eq!(
        note_with(&rig, "hover_class "),
        format!("hover_class location {}", ringforts[2].0)
    );
    // The third shown row is the list's fourth row (the header is first): a click opens that ringfort.
    click_ledger_row(&mut rig, 4, 1800, 900);
    rig.opened();
    assert_eq!(
        note_with(&rig, "last_place_call "),
        format!("last_place_call {}@1", ringforts[2].0)
    );
}

#[test]
fn the_people_sort_puts_the_most_first_and_the_places_without_people_last_in_both_directions() {
    // Every shipped place has people, so two of them get views with no pops (no people cell, no value to sort by).
    let ringforts = shipped_places_of("ringfort");
    let extra = format!(
        r#"
local real_actual_place = window.actual_place
EMPTY = {{["{}"] = true, ["{}"] = true}}
window.actual_place = function(id, revision)
  local view = real_actual_place(id, revision)
  if EMPTY[id] then view.pops = {{}} end
  return view
end
"#,
        ringforts[0].0, ringforts[4].0
    );
    let mut rig = shipped_ledger_rig_with(
        "location",
        &extra,
        &[
            "announce_ledger() announce_bar()",
            "announce_picker()",
            "check_people_order(true) announce_picker()",
            "check_people_order(false) announce_picker()",
            "announce_bar()",
        ],
    );
    rig.opened();
    let initial = note_with(&rig, "shown_ids ");
    click(&mut rig, "bar_sort");
    rig.opened();
    click_pick(&mut rig, "people");
    rig.opened();
    let order = note_with(&rig, "people_order ");
    assert!(
        order.starts_with("people_order with=92 without=2 "),
        "the two places with no people are last: {order}"
    );
    click_pick(&mut rig, "people");
    rig.opened();
    let again = note_with(&rig, "people_order ");
    assert!(
        again.starts_with("people_order with=92 without=2 "),
        "still last, with the fewest first: {again}"
    );
    // "default" restores the file order.
    click_pick(&mut rig, "default");
    rig.opened();
    assert_eq!(note_with(&rig, "shown_ids "), initial);
}

#[test]
fn the_pops_tab_filters_by_type_and_culture_together_sorts_by_place_and_its_rows_still_open_their_places_pops()
 {
    let mut rig = shipped_ledger_rig(
        "pops",
        &[
            "announce_ledger() announce_bar() local first = last_given[1] note('first_pop|' .. first.sort.type .. '|' .. first.sort.culture)",
            "announce_picker()",
            r#"
local want_type, want_culture = last_given[1].sort.type, last_given[1].sort.culture
local count = 0
for _, row in ipairs(last_given) do
  if row.sort.type == want_type and row.sort.culture == want_culture then count = count + 1 end
end
eq_that(#last_shown, count, "only the pops with both")
for _, row in ipairs(last_shown) do
  expect_that(row.sort.type == want_type and row.sort.culture == want_culture, "a pop without both is shown")
end
expect_that(count < #last_given, "the two ticks narrowed the list")
note("both " .. count)
announce_bar()
"#,
            "announce_picker()",
            r#"
for index = 2, #last_shown do
  expect_that(last_shown[index - 1].sort.place <= last_shown[index].sort.place, "place order " .. index)
end
-- The first shown row: its people cell's hover source is that pop's `pop_size`, and a click opens that place's pops tab.
list_object.items[1].tooltip(0, 0, 2)
local row = last_shown[1]
note("pop_hover " .. last_content.class .. " " .. tostring(last_content.args.place) .. " " .. tostring(last_content.args.pop))
note("first_place " .. row.place.id .. " " .. row.place.name)
"#,
            r#"
local row = last_shown[1]
local panel = window_objects[row.place.name]
note("panel_tab " .. tostring(panel ~= nil and tabs_by_title[row.place.name]:current()))
"#,
        ],
    );
    rig.opened();
    let first = note_with(&rig, "first_pop|");
    let mut parts = first.split('|').skip(1);
    let (kind, culture) = (
        parts.next().unwrap().to_owned(),
        parts.next().unwrap().to_owned(),
    );
    click(&mut rig, "bar_filter");
    rig.opened();
    let picker = note_with(&rig, "picker ");
    assert!(
        picker.starts_with("picker filter type,")
            && picker.contains(",religion,")
            && picker.contains(",culture,"),
        "the three groups, each under its heading: {picker}"
    );
    click_pick(&mut rig, &kind.replace('_', " "));
    click_pick(&mut rig, &culture.replace('_', " "));
    rig.opened();
    // Sort by place: the sort picker's fourth row.
    click(&mut rig, "bar_sort");
    rig.opened();
    assert_eq!(
        note_with(&rig, "picker "),
        "picker sort default,type,culture,people,place"
    );
    click_pick(&mut rig, "place");
    rig.opened();
    let hover = note_with(&rig, "pop_hover ");
    assert!(hover.starts_with("pop_hover pop_size "), "{hover}");
    click_ledger_row(&mut rig, 1, 1800, 900);
    rig.opened();
    assert_eq!(note_with(&rig, "panel_tab "), "panel_tab pops");
}

fn shown_scroll(rig: &Rig) -> u32 {
    note_with(rig, "shown ")
        .rsplit("scroll=")
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn a_ticked_box_stays_ticked_across_a_new_economy_revision_and_a_new_window_size() {
    let mut rig = shipped_ledger_rig(
        "location",
        &[
            "announce_ledger() announce_bar()",
            "announce_picker()",
            "announce_bar()",
            "time(1, 2) announce_bar() note('last_call ' .. place_calls[#place_calls])",
            "announce_bar()",
        ],
    );
    rig.opened();
    click(&mut rig, "bar_filter");
    rig.opened();
    click_pick(&mut rig, "ringfort");
    rig.opened();
    assert!(note_with(&rig, "shown ").starts_with("shown 10 of 94"));
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=1 sort=nil bars=1"
    );
    rig.opened();
    assert!(
        note_with(&rig, "last_call ").ends_with("@2"),
        "the rows were refreshed for revision 2: {:?}",
        notes(&rig)
    );
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=1 sort=nil bars=1"
    );
    assert!(
        note_with(&rig, "shown ").starts_with("shown 10 of 94"),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(note_with(&rig, "count "), "count 10 of 94");
    // A new window size closes the open picker, and the choices stay (the same bar, rebuilt around).
    rig.deliver_fixture_frame_at(1500, 900, 2);
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=1 sort=nil bars=1"
    );
    assert!(
        note_with(&rig, "shown ").starts_with("shown 10 of 94"),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(note_with(&rig, "count "), "count 10 of 94");
}

#[test]
fn the_sort_and_the_scroll_stay_across_a_revision_a_change_starts_the_list_from_its_first_row_and_a_tab_or_reopening_starts_again()
 {
    let mut rig = shipped_ledger_rig(
        "location",
        &[
            "announce_ledger() announce_bar()",
            "announce_picker()",
            "announce_bar()",
            "announce_bar()",
            "time(1, 2) announce_bar()",
            "announce_picker()",
            "announce_bar()",
            "announce_bar()",
            "announce_ledger_tabs()",
            "announce_bar() announce_ledger_tabs()",
            "announce_bar()",
            "bar_object:choose(2) announce_bar() announce_ledger()",
            "announce_ledger() announce_bar()",
        ],
    );
    rig.opened();
    click(&mut rig, "bar_sort");
    rig.opened();
    click_pick(&mut rig, "people");
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=2 bars=1"
    );
    assert_eq!(shown_scroll(&rig), 0);
    // The list scrolls under the wheel (the picker stays open: a wheel is not a press).
    let (origin_x, origin_y) = ledger_origin(&rig);
    rig.feed(&[scroll_lines(origin_x + 20.0, origin_y + 45.0, -3)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    let scrolled = shown_scroll(&rig);
    assert!(scrolled > 0, "{:?}", notes(&rig));
    // A new economy revision refreshes the rows; the choice and the scroll stay, and so does the open picker.
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=2 bars=1"
    );
    assert_eq!(
        shown_scroll(&rig),
        scrolled,
        "the scroll stays across a revision"
    );
    // A change (the chosen row pressed again) starts the list from its first row.
    rig.opened();
    click_pick(&mut rig, "people");
    rig.opened();
    assert_eq!(
        shown_scroll(&rig),
        0,
        "after a change the list shows from its first row"
    );
    // A new window size keeps the choice.
    rig.deliver_fixture_frame_at(1500, 900, 2);
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=2 bars=1"
    );
    // The tab changes: a new bar, nothing chosen; then back, a new bar again.
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=nil bars=2"
    );
    click_tab(&mut rig, "locations");
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=nil bars=3"
    );
    // Chosen again, then the ledger is closed and opened: a new bar.
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=2 bars=3"
    );
    assert!(rig.escape(), "Escape closes the ledger window");
    press_bar(&mut rig, "location");
    rig.opened();
    assert_eq!(
        note_with(&rig, "bar_state "),
        "bar_state ticks=0 sort=nil bars=4"
    );
}
