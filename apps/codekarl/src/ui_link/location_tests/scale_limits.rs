use super::*;

#[test]
fn card_scrolling_keeps_fractional_points_and_snaps_only_the_drawn_offset() {
    let mut rig = rig(&[
        "time(1, 1) select_place('scrolling', 'Scrolling')",
        "announce_tabs()",
        "card_object:set_pos(26); local start_y = card_object.shown_y; \
         card_object:scroll({amount=1, unit='points'}); card_object:scroll({amount=1, unit='points'}); \
         note('scroll_down ' .. card_object:scroll_position() .. ' ' .. card_object.shown_y - start_y); \
         card_object:scroll({amount=-1, unit='points'}); card_object:scroll({amount=-1, unit='points'}); \
         note('scroll_up ' .. card_object:scroll_position() .. ' ' .. card_object.shown_y - start_y); \
         card_object:scroll({amount=-0.5, unit='points'}); note('fractional ' .. card_object:scroll_position()); \
         card_object:scroll({amount=0.25, unit='points'}); note('fractional_2 ' .. card_object:scroll_position()); \
         card_object:set_pos(26); card_object:scroll({amount=-1, unit='lines'}); note('line_scroll ' .. card_object:scroll_position()); \
         card_object:set_items({card_object.items[1]}, 3); note('shrink_scroll ' .. card_object:scroll_position()); \
         local key = next(card_object.item_keys); note('card_grid ' .. key .. ' ' .. card_object.shown_x .. ' ' .. card_object.shown_y)",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded.iter().any(|line| line == "scroll_down 24 2"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "scroll_up 26 0"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "fractional 26.5"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "fractional_2 26.25"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "line_scroll 52"),
        "{recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "shrink_scroll 0"),
        "{recorded:?}"
    );

    let grid = recorded
        .iter()
        .find(|line| line.starts_with("card_grid "))
        .expect("card hit geometry");
    let mut fields = grid.split_whitespace();
    let _ = fields.next();
    let key = fields.next().unwrap();
    let x: f64 = fields.next().unwrap().parse().unwrap();
    let y: f64 = fields.next().unwrap().parse().unwrap();
    assert!(x.rem_euclid(2.0).abs() < f64::EPSILON, "{grid}");
    assert!(y.rem_euclid(2.0).abs() < f64::EPSILON, "{grid}");
    assert_eq!(rig.key_at(x + 2.0, y + 2.0).as_deref(), Some(key));
}

/// A window opened in pixels: the UI VM is in UI units at the scale its drawable allows, so the same 1280 by 800 view comes from
/// 1280 by 800 pixels at scale 1 and from 1920 by 1200 at 1.5, and the shipped script's relayout keeps the map-mode bar at the bottom
/// centre of it; a window of 3456 by 2232 pixels at scale 2 has a 1728 by 1116 view.
#[test]
fn a_change_of_the_ui_scale_is_seen_as_a_resize_and_the_script_relays_out_in_units() {
    let mut rig = Rig::new_fixture_body_in_pixels(&script(&[
        "time(1, 1) announce_bars()",
        "announce_bars()",
        "announce_bars()",
    ]));
    // 640x400 points at 2 pixels a point: a drawable of 1280x800 and scale 1: a view of 1280x800 units. The bar is 110x38 units, centred
    // (585, snapped to 586) and 8 above the bottom edge (800 - 38 - 8 = 754).
    rig.deliver_fixture_frame_at(640, 400, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [586, 754]);
    // 960x600 points: 1920x1200 pixels, the scale 1.5 and the same 1280x800 view: the layout is unchanged.
    rig.deliver_fixture_frame_at(960, 600, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [586, 754]);
    // 1728x1116 points: 3456x2232 pixels, the scale 2 and a 1728x1116 view: (1728 - 110) / 2 = 809 snaps to 810; 1116 - 38 - 8 = 1070.
    rig.deliver_fixture_frame_at(1728, 1116, 2);
    rig.opened();
    assert_mode_bar_at(&rig, [810, 1070]);
}

/// At 3456 by 2232 pixels on a 2x display the scale is 2 and a UI unit is a point: every quad of the shipped start is where the build with
/// a UI pixel of 2 points drew it, in the units and on the pixels.
#[test]
fn at_the_scale_two_the_shipped_start_lands_on_the_pixels_of_a_points_window() {
    let steps = ["time(1, 1) select_place('rowan', 'Rowan')"];
    let mut points = Rig::new_fixture_body(&script(&steps));
    let mut pixels = Rig::new_fixture_body_in_pixels(&script(&steps));
    for rig in [&mut points, &mut pixels] {
        rig.deliver_fixture_frame_at(1728, 1116, 2);
        rig.opened();
    }
    let rects = |rig: &Rig| {
        rig.sink
            .quads()
            .iter()
            .map(|quad| (quad.x, quad.y, quad.width, quad.height))
            .collect::<Vec<_>>()
    };
    assert!(rects(&points).len() > 20, "the start has quads");
    assert_eq!(rects(&points), rects(&pixels), "the same units");
    let drawn = |rig: &Rig, scale_k: Option<u32>| {
        let metrics = ck_render_2d::Metrics {
            points_width: 1728,
            points_height: 1116,
            pixels: ck_window::PixelSize {
                width: 3456,
                height: 2232,
            },
            ui_scale_k: scale_k,
        };
        rig.sink
            .quads()
            .iter()
            .map(|quad| {
                quad.place(&metrics)
                    .map(|placed| format!("{:?}", placed.destination))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        drawn(&points, None),
        drawn(&pixels, Some(4)),
        "the same pixels"
    );
}

#[test]
fn strict_limit_markers_reach_place_rows_classes_and_wrapping() {
    let mut rig = rig(&[r#"local place = VIEWS[1].rowan
place.crowding_pm = 1120
place.basic_housing_occupants, place.basic_housing_capacity = 310, 300
place.makeshift_housing_occupants, place.makeshift_housing_capacity = 40, 60
place.footprint_m2, place.prepared_m2, place.max_buildable_m2 = 9100, 9000, 9500
place.storage_used_l, place.storage_capacity_l, place.overflow_lost = 60010, 60000, 52
place.last_month.lost = {{good = "oats", units = 52}}
place.pops[1].crowding_pm = 1120
place.buildings[1].store_used_l, place.buildings[1].store_capacity_l = 60010, 60000
colour_watch = true
time(1, 1); select_place("rowan", "Rowan")
local location = require("lib.location")
local wrapped = location.rows(location.lines("location", game.place("rowan", 1)), 204)
for _, row in ipairs(wrapped) do for _, cell in ipairs(row.cells or {}) do
  if cell.over_limit then note("wrapped_limit " .. cell.text) end
  if cell.text:find("310", 1, true) then note("wrapped_numerator " .. tostring(cell.over_limit) .. " " .. cell.text) end
  if cell.text:find("300", 1, true) then note("wrapped_capacity " .. tostring(cell.over_limit) .. " " .. cell.text) end
end end
local tip = require("lib.tooltip")
local function marked(name, content)
  for _, line in ipairs(content or {}) do if type(line) == "table" then
    for _, span in ipairs(line) do
      if type(span) == "table" and span.over_limit then note("class_limit " .. name .. " " .. span.text) end
    end
  end end
end
for _, name in ipairs({"crowding", "pop_crowding", "housing", "storage", "store", "building_land", "overflow"}) do
  marked(name, tip.content(name, {place = "rowan", building = 1, pop = 1}))
end"#]);
    rig.opened();
    let recorded = notes(&rig);
    for (value, ink) in [
        ("112%", "#f66155"),
        ("310", "#f66155"),
        ("300", "#5fb35a"),
        ("40", "#5fb35a"),
        ("9100", "#f66155"),
        ("9000", "#5fb35a"),
        ("60010", "#f66155"),
        ("60000", "#5fb35a"),
        ("52", "#f66155"),
    ] {
        assert!(
            recorded
                .iter()
                .any(|line| line.contains(&format!("{ink}={value}"))),
            "{value} should use {ink}: {recorded:?}"
        );
    }
    for expected in [
        "class_limit crowding 112%",
        "class_limit pop_crowding 112%",
        "class_limit housing 310",
        "class_limit storage 60010",
        "class_limit store 60010",
        "class_limit building_land 9100",
        "class_limit overflow 52",
    ] {
        assert!(
            recorded.iter().any(|line| line == expected),
            "missing {expected}: {recorded:?}"
        );
    }
    assert!(
        recorded
            .iter()
            .any(|line| line == "wrapped_numerator true 310"),
        "the wrapped numerator keeps the over-limit marker: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "wrapped_capacity false /300"),
        "the wrapped denominator does not inherit the numerator marker: {recorded:?}"
    );
}

#[test]
fn values_at_their_limits_and_zero_overflow_keep_number_colour() {
    let mut rig = rig(&[r#"local place = VIEWS[1].rowan
place.crowding_pm = 1000
place.basic_housing_occupants, place.basic_housing_capacity = 300, 300
place.makeshift_housing_occupants, place.makeshift_housing_capacity = 40, 60
place.footprint_m2, place.prepared_m2, place.max_buildable_m2 = 9000, 9000, 9500
place.storage_used_l, place.storage_capacity_l, place.overflow_lost = 60000, 60000, 0
place.last_month.lost = {}
colour_watch = true; time(1, 1); select_place("rowan", "Rowan")"#]);
    rig.opened();
    let recorded = notes(&rig);
    for value in ["100%", "300", "60000", "9000", "0"] {
        assert!(
            recorded
                .iter()
                .any(|line| line.contains(&format!("#5fb35a={value}"))),
            "at-limit or zero value {value} stays in number colour: {recorded:?}"
        );
    }
}

#[test]
fn normalized_tooltip_spans_colour_only_numbers_marked_past_the_limit() {
    let mut rig = rig(&[r#"colour_watch = true
local text = require("lib.text")
local real_set = text.set
text.set = function(line, value)
  real_set(line, value)
  if line.font_name ~= nil then note("tooltip_span " .. line.colour .. "=" .. value) end
end
local tip = require("lib.tooltip")
tip.attach("ck283_limit_span", function()
  return {{{text = "112%", over_limit = true}, " / ", "100%"}}
end)
tip.update({width = 1800, height = 900, dt = 1,
  pointer = {over = "ck283_limit_span", x = 500, y = 200, events = {}}})"#]);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_span #f66155=112%"),
        "the normalized over-limit span colours its numeric run: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_span #ece3cb= / "),
        "ordinary tooltip text keeps its normal ink: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_span #5fb35a=100%"),
        "unmarked tooltip numbers remain green: {recorded:?}"
    );
}
