use super::*;

#[test]
fn a_place_of_nobody_has_zeroes_and_empty_pops_and_buildings() {
    let mut rig = rig(&[
        "time(1, 1) select_place('nobody', 'Nobody')",
        "announce_tabs()",
    ]);
    rig.opened();
    assert_eq!(
        rows(&rig),
        [
            "people: 0|  workers: 0|  dependents: 0|housing: basic 0/0, makeshift 0/0|crowding: 0%|food: 0 days|building land: 0 / 0 m²|storage: 0 / 0 L|lost to overflow last month: 0|good@16:left:#92ddea,price@212:right:#92ddea,base@238:right:#92ddea,buy/sell@246:left:#9a927e;stripe=-;icon=-"
        ]
    );
    rig.opened();
    click_tab(&mut rig, "pops");
    click_tab(&mut rig, "buildings");
    assert_eq!(
        card_stacks(&rig),
        ["gap=3 heights=13".to_owned(), "gap=3 heights=13".to_owned()]
    );
}

#[test]
fn the_panel_refreshes_on_a_new_revision_and_not_on_a_date_alone() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan')",
        // Only the date moves: the same revision.
        "time(2, 1) time(3, 1)",
        // A new revision refreshes the open panel from its own view.
        "time(12, 2)",
        "note('calls ' .. table.concat(place_calls, ','))",
    ]);
    rig.opened();
    rig.opened();
    assert_eq!(
        rows(&rig),
        [LOCATION_ROWS],
        "a date alone refreshes nothing"
    );
    rig.opened();
    let refreshed = LOCATION_ROWS.replace("people: 120|  workers: 80", "people: 130|  workers: 90");
    assert_eq!(rows(&rig), [LOCATION_ROWS.to_owned(), refreshed]);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("calls rowan@1,rowan@2"),
        "one read for the selection and one for the new revision, none for the dates"
    );
}

#[test]
fn the_panel_fills_the_left_third_below_the_time_bar_and_follows_a_resize() {
    let mut rig = rig(&[
        "time(1, 1) select_place('rowan', 'Rowan') announce_geometry()",
        "announce_geometry()",
    ]);
    rig.opened();
    // 1800 points wide: the left third is 600 points; the time bar is 8 + 38 points down, and the panel sits a margin below it.
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("panel 0 54 600 846")
    );
    rig.deliver_fixture_frame_at(1200, 700, 2);
    rig.opened();
    assert_eq!(
        notes(&rig).last().map(String::as_str),
        Some("panel 0 54 400 646")
    );
}

#[test]
fn a_window_taller_than_the_lists_row_limit_opens_and_resizes_with_a_usable_list() {
    let mut rig = Rig::new_fixture_body(&script(&[
        "time(1, 1) select_place('rowan', 'Rowan') announce_fit('Rowan')",
        "announce_fit('Rowan')",
    ]));
    // A row is 13 UI pixels, 26 points: 7,200 points leave room for more than the limit of 256 rows.
    rig.deliver_fixture_frame_at(1800, 7200, 1);
    rig.opened();
    assert_eq!(
        notes(&rig)
            .iter()
            .rev()
            .find(|line| line.starts_with("list_rows "))
            .map(String::as_str),
        Some("list_rows 256")
    );
    // A resize that keeps it that tall rebuilds the list without failing.
    rig.deliver_fixture_frame_at(1700, 7300, 1);
    rig.opened();
    assert_eq!(
        notes(&rig)
            .iter()
            .rev()
            .find(|line| line.starts_with("list_rows "))
            .map(String::as_str),
        Some("list_rows 256")
    );
}

/// The narrow layouts: below the least width that holds the tabs, the close button and the frame the panel is that width and extends
/// past the window; nothing raises, the title is empty when it has no room, and every control lies inside the panel, apart from the
/// others and clear of the title.
fn narrow_layout(width: u32, expect_floor: bool) {
    const NAME: &str = "Droim Ineasclainn";
    let scale = 2;
    let mut rig = Rig::new_fixture_body(&script(&[&format!(
        "time(1, 1) select_place('rowan', '{NAME}') announce_floor() announce_geometry() announce_fit('{NAME}')"
    )]));
    rig.deliver_fixture_frame_at(width, 800, 2);
    rig.opened();
    let line = |rig: &Rig, prefix: &str| {
        notes(rig)
            .into_iter()
            .rev()
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix}: {:?}", notes(rig)))
    };
    let floor: i64 = line(&rig, "floor ")
        .trim_start_matches("floor ")
        .parse()
        .unwrap();
    let panel: Vec<i64> = line(&rig, "panel ")
        .split_whitespace()
        .skip(1)
        .map(|part| part.parse().unwrap())
        .collect();
    let third = i64::from(width) / 3;
    assert!(
        panel[2] >= floor && panel[2] >= third - scale,
        "{panel:?} floor {floor}"
    );
    if expect_floor {
        assert!(
            floor > third,
            "the floor is above the third: {floor} > {third}"
        );
        assert_eq!(
            panel[2], floor,
            "the panel is its floor wide, and passes the window"
        );
    }
    // The whole name is kept; the tabs are inside the panel, apart from each other and the close button, and clear of the title.
    assert_eq!(
        line(&rig, "fit "),
        "fit title=true tabs_right=true title_clear=true"
    );
    assert_eq!(line(&rig, "tabs_left "), "tabs_left true");
    assert_eq!(line(&rig, "apart "), "apart true");
    if expect_floor {
        assert_eq!(line(&rig, "shown "), "shown ", "no room: no title");
    }
}

#[test]
fn in_a_narrow_window_the_panel_is_its_floor_and_passes_the_window() {
    narrow_layout(200, true);
}

#[test]
fn in_a_window_wide_enough_the_third_holds_the_controls() {
    narrow_layout(900, false);
}

/// The shipped script opens the window resizable, at 1280 by 800 screen pixels, with that minimum (milestone 9, slice 12b): this
/// rig has no screen snapshot, so the pixels resolve one to one and the description keeps the request it was made from.
#[test]
fn the_shipped_script_opens_a_resizable_window_of_1280_by_800_pixels_with_that_minimum() {
    let rig = rig(&["time(1, 1)"]);
    let desc = rig
        .harness
        .requests
        .reader()
        .take()
        .open
        .expect("the shipped script opened its window");
    assert!(desc.resizable);
    assert_eq!(desc.unit, ck_window::SizeUnit::Points, "resolved");
    assert_eq!((desc.width, desc.height), (1280, 800));
    assert_eq!(desc.min, Some((1280, 800)));
    assert_eq!(
        desc.pixel_request,
        Some(ck_window::PixelRequest {
            size: (1280, 800),
            min: Some((1280, 800)),
        }),
        "asked for in pixels, with the minimum in pixels"
    );
}

#[test]
fn startup_viewport_uses_the_window_open_size_before_first_layout_and_resize() {
    let main_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let main = std::fs::read_to_string(main_path).expect("the production UI script");
    let body = format!(
        r#"
{PREFIX}
local real_open = window.open
window.open = function(spec)
  local width, height = real_open(spec)
  opened_width, opened_height = width, height
  return width, height
end
{main}
time(0, 1)
note(string.format("open_size %d %d", opened_width, opened_height))
local windows = require("lib.window")
local probe = windows.new{{title="viewport probe", content_width=100 * P, content_height=50 * P}}
local x, y = probe:position()
local width, height = probe:size()
local expected_x = require("lib.ui").snap((opened_width - width) / 2)
local expected_y = require("lib.ui").snap((opened_height - height) / 2)
note(string.format("viewport_center %g %g %g %g", x, y, expected_x, expected_y))
local visible = math.min(16 * P, width)
local max_x = math.max(visible - width, math.floor((opened_width - visible) / P) * P)
local max_y = math.max(0, math.floor((opened_height - probe.title_height) / P) * P)
probe:move_to(1000000, 1000000)
x, y = probe:position()
note(string.format("viewport_drag %g %g %g %g", x, y, max_x, max_y))
window.on_opened(function()
  if window_objects.ledger ~= nil and not window_objects.ledger.closed then announce_ledger()
  else announce_bars() end
end)
"#
    );
    let mut rig = Rig::new_fixture_body(&body);
    let open = notes(&rig)
        .into_iter()
        .find(|line| line.starts_with("open_size "))
        .expect("window.open's resolved size");
    let mut fields = open.split_whitespace().skip(1);
    let width: u32 = fields.next().unwrap().parse().unwrap();
    let height: u32 = fields.next().unwrap().parse().unwrap();

    // This is the first frame, exactly the size returned from window.open: no resize callback can repair a stale viewport.
    rig.deliver_fixture_frame_at(width, height, 1);
    rig.opened();
    for prefix in ["viewport_center ", "viewport_drag "] {
        let line = notes(&rig)
            .into_iter()
            .find(|line| line.starts_with(prefix))
            .unwrap();
        let values: Vec<f64> = line[prefix.len()..]
            .split_whitespace()
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(&values[..2], &values[2..], "{line}");
    }

    let button = bar_button(&rig, "location");
    let point = (
        point_value(button[0] + button[2] / 2),
        point_value(button[1] + button[3] / 2),
    );
    rig.feed(&[
        PointerEvent::Down(points(point.0, point.1)),
        PointerEvent::Up(points(point.0, point.1)),
    ]);
    rig.deliver_fixture_frame_at(width, height, 1);
    rig.opened();
    let opened = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the ledger geometry at the opened size");
    let mut geometry = opened.split_whitespace();
    assert_eq!(geometry.next(), Some("ledger"));
    let x: f64 = geometry.next().unwrap().parse().unwrap();
    let _y: f64 = geometry.next().unwrap().parse().unwrap();
    let panel_width: f64 = geometry.next().unwrap().parse().unwrap();
    assert!(
        (x + panel_width - f64::from(width)).abs() < f64::EPSILON,
        "{opened}"
    );

    rig.deliver_fixture_frame_at(width + 200, height + 100, 1);
    rig.opened();
    let resized = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("ledger "))
        .expect("the ledger geometry after resize");
    let mut geometry = resized.split_whitespace();
    let _ = geometry.next();
    let x: f64 = geometry.next().unwrap().parse().unwrap();
    let _y: f64 = geometry.next().unwrap().parse().unwrap();
    let panel_width: f64 = geometry.next().unwrap().parse().unwrap();
    assert!(
        (x + panel_width - f64::from(width + 200)).abs() < f64::EPSILON,
        "{resized}"
    );
}

/// CK-299: the location panel's floor is 5 UI pixels (10 points) wider, 118 points where it was 108, so that the market's columns
/// (the balance bar is 46 wide from 48 pixels before the row's right edge) fit beside the list's scroll bar. At the floor the
/// overflowing market's bar starts at 0, whole, and nothing is refused.
#[test]
fn at_the_new_floor_an_overflowing_market_has_its_balance_bar_whole_beside_the_scroll_bar() {
    let mut rig = rig_with_prelude_at(
        PANEL_BARS,
        &[r"
local function eq_that(actual, wanted, message)
  if actual ~= wanted then error(message .. ': got ' .. tostring(actual) .. ', wanted ' .. tostring(wanted), 2) end
end
local goods = {}
for i = 1, 60 do goods[i] = price('good' .. i, 1000 + i * 10, 5, 10, 20) end
VIEWS[1].rowan.prices = goods
time(1, 1); select_place('rowan', 'Rowan')
eq_that(require('lib.location').min_width(), 118, 'the floor was 108 points')
local window = panel_bar.window
eq_that(window.outer_width, 118, 'the panel is the floor wide')
eq_that(window.content_width, 106)
local px = window.content_width // 2
eq_that(px, 53, 'the content is 53 pixels: the 48 of the market and the 5 of the bar')
local overflowing = #list_object.items * 26 > list_object.view_height
eq_that(overflowing, true, 'the goods overflow the list')
for _, item in ipairs(list_object.items) do
  if item.bar ~= nil then
    eq_that(item.bar.x, 0, 'the balance bar starts at the row width less 48, which is 0 here')
    eq_that(item.bar.width, 46, 'and is whole')
    eq_that(item.bar.x + item.bar.width <= px - 5, true, 'clear of the gap and the track')
  end
end
note('floor ok')"],
        200,
        800,
        2,
    );
    rig.opened();
    assert!(notes(&rig).iter().any(|line| line == "floor ok"));
}
