use super::*;

#[test]
fn herd_counts_use_full_colour_icons_on_cards_and_open_rows() {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)",
        "look_buttons.rows.on_click(); card_object.items[2].on_click()",
    ]);
    rig.opened();
    let card_icons = notes(&rig);
    for (name, icon_x, count_x, colour, count) in [
        ("cattle", 10, 32, "#9a927e", "27/36"),
        ("sheep", 112, 134, "#9a927e", "10/20"),
        ("pigs", 214, 236, "#ece3cb", "64/64"),
    ] {
        assert!(
            card_icons.iter().any(|line| {
                line == &format!("goodsquad ui/goods/{name}.png {icon_x} 76 18 18 -")
            }),
            "card herd {name} icon is at ({},38) UI pixels, untinted 9x9: {card_icons:?}",
            icon_x / 2
        );
        assert!(
            card_text_with_colour_at(&card_icons, count_x, 74, colour, count),
            "card herd {name} count is at ({},37) UI pixels with {colour}: {card_icons:?}",
            count_x / 2
        );
    }
    assert_eq!(
        card_run_count_at(&card_icons, 236, 74, "#5fb35a", "64"),
        2,
        "both maximum herd digits are green: {card_icons:?}"
    );
    assert!(
        card_run_at(&card_icons, 236, 74, "#ece3cb", "/"),
        "maximum herd separator keeps text colour: {card_icons:?}"
    );
    rig.opened();
    let row_icons = notes(&rig);
    for (name, icon_y, count_y, count) in [
        ("cattle", 104, 102, "27 / 36"),
        ("sheep", 128, 126, "10 / 20"),
        ("pigs", 152, 150, "64 / 64"),
    ] {
        assert!(
            row_icons.iter().any(|line| {
                line == &format!("goodsquad ui/goods/{name}.png 32 {icon_y} 18 18 -")
            }),
            "open row herd {name} icon is at x=16 and one pixel below its text: {row_icons:?}"
        );
        assert!(
            card_text_with_colour_at(&row_icons, 56, count_y, "#ece3cb", count),
            "open row herd {name} count is at x=28: {row_icons:?}"
        );
    }
}

#[test]
fn unlisted_herds_use_the_unknown_icon_on_cards_and_open_rows() {
    let mut rig = rig(&[
        r"VIEWS[1].cards.prices = {price('moonstone', 4500, 4), price('quartz', 2500, 3)}
VIEWS[1].cards.buildings[1].herds = {
  {species = 'cattle', head = 27, maximum = 36}, {species = 'goats', head = 5, maximum = 8}
}
time(1, 1); select_place('cards', 'Cards'); tabs_by_title.Cards:select('buildings', true)
local building = card_object.items[1]
local function text(value)
  if type(value) ~= 'table' then return value == nil and '<none>' or tostring(value) end
  if type(value.text) == 'string' then return value.text end
  local result = {}
  for _, part in ipairs(value) do result[#result + 1] = text(part) end
  return table.concat(result)
end
local function tip(name, x, y)
  local content, part = building.tooltip(x, y)
  note(name .. ' ' .. text(content and content.title or content) .. ' part=' .. tostring(part or '-'))
end
local goats_x = 5 + 11 + font.layout('27/36').width + 10
tip('goats_icon_card', goats_x + 1, 42); tip('goats_count_card', goats_x + 12, 42)
look_buttons.rows.on_click()
local row = card_object.items[2]
row.on_click()
row = card_object.items[2]
local function row_tip(name, x, y)
  local content, part = row.tooltip(x, y)
  note(name .. ' ' .. text(content and content.title or content) .. ' part=' .. tostring(part or '-'))
end
row_tip('goats_icon_row', 20, 50); row_tip('goats_count_row', 30, 50)",
    ]);
    rig.opened();
    let card = notes(&rig);
    assert!(
        card.iter()
            .any(|line| line == "goodsquad ui/goods/fallback/unknown.png 112 76 18 18 -"),
        "the card herd icon uses the fallback at the normal location and size: {card:?}"
    );
    assert!(
        card.iter()
            .any(|line| line == "goats_icon_card <none> part=goats")
    );
    assert!(
        card.iter()
            .any(|line| line == "goats_count_card goats 5 of 8 part=goats"),
        "unknown herd count retains the herd class: {card:?}"
    );
    assert!(
        card.iter()
            .any(|line| line == "goodsquad ui/goods/fallback/unknown.png 32 128 18 18 -"),
        "the open-row herd icon uses the fallback at the normal location and size: {card:?}"
    );
    assert!(
        card.iter()
            .any(|line| line == "goats_icon_row <none> part=goats")
    );
    assert!(
        card.iter()
            .any(|line| line == "goats_count_row goats 5 of 8 part=goats")
    );
    assert!(
        !card.iter().any(|line| line.contains("ui/goods/goats.png")),
        "no per-species texture is loaded: {card:?}"
    );
    assert_eq!(
        card.iter()
            .filter(|line| line.as_str() == "goodsload ui/goods/fallback/unknown.png")
            .count(),
        1,
        "two unknown goods and a herd share one cached fallback texture: {card:?}"
    );
}

const BUILDING_CARD_LAYOUT_PROBE: &str = r##"
local P = require("lib.ui").P
local building = VIEWS[1].cards.buildings[1]
local item = card_object.items[1]
local captured, record = {}, note
note = function(value) captured[#captured + 1] = value; record(value) end
local p, s = font.layout("positions").width, font.layout("storage").width
local function layout(width)
  local count = building.filled .. "/" .. (building.filled + building.open)
  local c = font.layout(count).width
  local fixed = 5 + p + 4 + 4 + c + 12 + s + 4 + 6
  local room = width - fixed
  local out = {step = 6, count = count, count_width = c}
  if room >= 34 then
    out.step, out.positions_x, out.positions_width = 1, 5, p
    out.positions_bar_x, out.positions_bar_width = 5 + p + 4, 24
    out.count_x = out.positions_bar_x + out.positions_bar_width + 4
    out.storage_x = out.count_x + c + 12
    out.storage_bar_x, out.storage_bar_width = out.storage_x + s + 4, room - 24
  elseif room >= 12 then
    out.step, out.positions_x, out.positions_width = 2, 5, p
    out.positions_bar_width = math.floor(room / 2)
    out.positions_bar_x = 5 + p + 4
    out.count_x = out.positions_bar_x + out.positions_bar_width + 4
    out.storage_x = out.count_x + c + 12
    out.storage_bar_x, out.storage_bar_width = out.storage_x + s + 4, room - out.positions_bar_width
  elseif width >= 5 + p + 4 + c + 12 + s + 6 then
    out.step, out.positions_x, out.positions_width = 3, 5, p
    out.count_x, out.storage_x = 5 + p + 4, 5 + p + 4 + c + 12
  elseif width >= 5 + p + 4 + c + 6 then
    out.step, out.positions_x, out.positions_width = 4, 5, p
    out.count_x = 5 + p + 4
  elseif width >= 5 + p + 6 then
    out.step, out.positions_x, out.positions_width = 5, 5, p
  end
  return out
end
local function tip(x, y)
  local content, part = item.tooltip(x, y)
  return content, part
end
local function expect_tip(x, y, expected_title, expected_part)
  local content, part = tip(x, y)
  assert(part == expected_part, "tooltip part at " .. x .. "," .. y .. ": " .. tostring(part))
  if expected_title ~= nil then
    assert(content ~= nil and content.title == expected_title,
      "tooltip title at " .. x .. "," .. y .. ": " .. tostring(content and content.title))
  elseif expected_part == nil then
    assert(content == nil, "unexpected tooltip at " .. x .. "," .. y .. " part=" .. tostring(part)
      .. " title=" .. tostring(content and content.title))
  end
end
local function draw_case(name, width, expected_step, filled, open)
  building.filled, building.open = filled or 80, open or 8
  local expected = layout(width)
  assert(expected.step == expected_step, name .. " expected step " .. expected_step .. " got " .. expected.step)
  local start = #captured + 1
  local draw_ok, drawn = pcall(item.draw, card_object.group, 0, 0, width * P)
  assert(draw_ok, name .. " draw failed: " .. tostring(drawn))
  local actual_text, actual_tracks = {}, {}
  for index = start, #captured do
    local line = captured[index]
    local x, y, line_width, colour, content = line:match("^cardtext ([%d%.]+) ([%d%.]+) (%d+) ([^ ]+) (.*)$")
    if x ~= nil and tonumber(y) == 50 then
      actual_text[#actual_text + 1] = {x = tonumber(x), width = tonumber(line_width), text = content}
      assert(tonumber(x) + tonumber(line_width) <= (width - 6) * P,
        name .. " text exceeds the right margin: " .. line)
    end
    local _, quad_x, quad_y, quad_width, quad_height, tint =
      line:match("^cardquad (%S+) ([%d%.]+) ([%d%.]+) ([%d%.]+) ([%d%.]+) (%S+)$")
    if quad_x ~= nil and tonumber(quad_y) == 60 and tonumber(quad_height) == 6 and tint == "#3a3326" then
      actual_tracks[#actual_tracks + 1] = {x = tonumber(quad_x), width = tonumber(quad_width)}
      assert(tonumber(quad_x) + tonumber(quad_width) <= (width - 6) * P,
        name .. " bar exceeds the right margin: " .. line)
      assert(tonumber(quad_width) >= 6 * P, name .. " bar is narrower than six UI pixels: " .. line)
    end
  end
  local expected_text = {}
  if expected.positions_x ~= nil then expected_text[#expected_text + 1] = {x = expected.positions_x, text = "positions"} end
  if expected.count_x ~= nil then expected_text[#expected_text + 1] = {x = expected.count_x, text = expected.count} end
  if expected.storage_x ~= nil then expected_text[#expected_text + 1] = {x = expected.storage_x, text = "storage"} end
  assert(#actual_text == #expected_text, name .. " third-line text count: " .. #actual_text .. " vs " .. #expected_text)
  for index, wanted in ipairs(expected_text) do
    local got = actual_text[index]
    assert(got.text == wanted.text and got.x == wanted.x * P,
      name .. " third-line text " .. index .. " expected " .. wanted.text .. " at " .. wanted.x * P
        .. ", got " .. got.text .. " at " .. got.x)
    assert(got.width == font.layout(wanted.text).width,
      name .. " uses the measured font width for " .. wanted.text .. ": got " .. got.width
        .. " expected " .. font.layout(wanted.text).width)
  end
  local expected_tracks = {}
  if expected.positions_bar_x ~= nil then
    expected_tracks[#expected_tracks + 1] = {x = expected.positions_bar_x, width = expected.positions_bar_width}
    expected_tracks[#expected_tracks + 1] = {x = expected.storage_bar_x, width = expected.storage_bar_width}
  end
  assert(#actual_tracks == #expected_tracks, name .. " track count: " .. #actual_tracks .. " vs " .. #expected_tracks)
  for index, wanted in ipairs(expected_tracks) do
    local got = actual_tracks[index]
    assert(got.x == wanted.x * P and got.width == wanted.width * P,
      name .. " track " .. index .. " geometry mismatch")
  end

  if expected.positions_x ~= nil then
    expect_tip(expected.positions_x + p // 2, 26, "position", "positions")
  else
    expect_tip(5, 26, nil, nil)
  end
  if expected.positions_bar_x ~= nil then
    expect_tip(expected.positions_bar_x + expected.positions_bar_width // 2, 31, nil, "staff_bar")
  else
    expect_tip(5 + p + 1, 31, nil, nil)
  end
  if expected.count_x ~= nil then
    expect_tip(expected.count_x + 1, 26, nil, "staff_count")
  else
    expect_tip(5 + p + 5, 26, nil, nil)
  end
  if expected.storage_x ~= nil then
    expect_tip(expected.storage_x + 1, 26, "storage", "storage")
  else
    if expected.count_x ~= nil then
      expect_tip(expected.count_x + expected.count_width + 1, 26, nil, nil)
    else
      expect_tip(5 + p + 2, 26, nil, nil)
    end
  end
  if expected.storage_bar_x ~= nil then
    expect_tip(expected.storage_bar_x + 1, 31, nil, "store_bar")
  else
    local after_label
    if expected.storage_x ~= nil then after_label = expected.storage_x + s + 1
    elseif expected.count_x ~= nil then after_label = expected.count_x + expected.count_width + 13
    else after_label = 5 + p + 2 end
    expect_tip(after_label, 31, nil, nil)
  end
  if drawn ~= nil and drawn.destroy ~= nil then drawn:destroy() end
end

local W = card_object.width // P
local ordinary = layout(W)
assert(ordinary.step == 1, "the normal 1280x800 card uses the approved full layout")
draw_case("approved-usual-12-16", W, 1, 12, 4)
draw_case("usual", W, 1)
local F = 5 + p + 4 + 4 + font.layout("80/88").width + 12 + s + 4 + 6
draw_case("full-minimum", F + 34, 1)
draw_case("shared-below-full", F + 33, 2)
draw_case("shared-minimum", F + 12, 2)
draw_case("no-bars", F + 11, 3)
local T4 = 5 + p + 4 + font.layout("80/88").width + 6
draw_case("no-storage", T4, 4)
draw_case("below-no-storage", T4 - 1, 5)
local T5 = 5 + p + 6
draw_case("positions-only", T5, 5)
draw_case("empty", T5 - 1, 6)
draw_case("minimum-empty", 5 + p + 5, 6)
draw_case("long-count", W, 2, 1000, 0)
building.filled, building.open = 80, 8
local min_width = require("lib.location").min_width()
local _, outer_height = panel_window:size()
panel_window:set_outer_size(min_width, outer_height)
local narrow_width = card_object.width // P
local narrow = layout(narrow_width)
draw_case("minimum-window", narrow_width, narrow.step)
note("ck284 width measurements " .. p .. " " .. s .. " " .. font.layout("80/88").width .. " " .. W
  .. " " .. narrow_width .. " " .. min_width)
"##;

#[test]
fn building_card_third_line_uses_real_widths_for_each_fallback_step() {
    let rig = rig_at(
        &[
            "time(1, 1) select_place('cards', 'Cards')",
            "announce_tabs(); tabs_by_title.Cards:select('buildings', true)",
            BUILDING_CARD_LAYOUT_PROBE,
        ],
        1280,
        800,
        2,
    );
    let mut rig = rig;
    rig.opened();
    rig.opened();
    rig.opened();
    let captured = notes(&rig);
    assert!(
        captured
            .iter()
            .any(|line| line.starts_with("ck284 width measurements ")),
        "measured font and card widths were recorded: {captured:?}"
    );
}

fn cards_rendered_fixture() -> Rig {
    let mut rig = rig(&[
        "time(1, 1) select_place('cards', 'Cards')",
        "announce_tabs()",
    ]);
    rig.opened();
    rig.opened();
    click_tab(&mut rig, "pops");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=61,61")
    );
    click_tab(&mut rig, "buildings");
    rig
}

fn cards_rendered_fixture_at(width: u32, height: u32, scale: u32) -> Rig {
    let mut rig = rig_at(
        &[
            "time(1, 1) select_place('cards', 'Cards')",
            "announce_tabs()",
        ],
        width,
        height,
        scale,
    );
    rig.opened();
    rig.opened();
    click_tab_at(&mut rig, "pops", (width, height));
    click_tab_at(&mut rig, "buildings", (width, height));
    rig
}

fn card_text_at(recorded: &[String], x: u32, y: u32, value: &str) -> bool {
    recorded
        .iter()
        .any(|line| line.starts_with(&format!("cardtext {x} {y} ")) && line.ends_with(value))
}

fn card_text_with_colour_at(
    recorded: &[String],
    x: u32,
    y: u32,
    colour: &str,
    value: &str,
) -> bool {
    let suffix = format!(" {colour} {value}");
    recorded
        .iter()
        .any(|line| line.starts_with(&format!("cardtext {x} {y} ")) && line.ends_with(&suffix))
}

fn card_run_count_at(recorded: &[String], x: u32, y: u32, colour: &str, value: &str) -> usize {
    let wanted = format!("cardrun {x} {y} {colour}={value}");
    recorded.iter().filter(|line| *line == &wanted).count()
}

fn card_quad_at(recorded: &[String], x: u32, y: u32, width: u32, colour: &str) -> bool {
    recorded.iter().any(|line| {
        line.starts_with("cardquad ") && line.contains(&format!(" {x} {y} {width} 6 {colour}"))
    })
}

#[test]
fn pop_cards_render_mockup_text_positions_and_mood() {
    let recorded = notes(&cards_rendered_fixture());
    for (x, y, value) in [
        (34, 6, "peasant"),
        (34, 28, "ulaid · christianity"),
        (10, 50, "food"),
        (146, 50, "74d"),
        (210, 50, "job"),
        (10, 74, "needs"),
        (146, 74, "mood"),
        (202, 74, "+12"),
        (10, 96, "housing"),
        (94, 96, "380 + 32"),
        (210, 96, "crowd"),
        (270, 96, "95%"),
    ] {
        assert!(
            card_text_at(&recorded, x, y, value),
            "{value} at ({x},{y}): {recorded:?}"
        );
    }
    assert!(card_text_at(&recorded, 270, 224, "—"));
    for (colour, value) in [
        ("#92ddea", "peasant"),
        ("#92ddea", "ulaid"),
        ("#92ddea", "christianity"),
        ("#92ddea", "food"),
        ("#5fb35a", "12"),
        ("#5fb35a", "3"),
        ("#ece3cb", "+"),
        ("#ece3cb", "-"),
        ("#9a927e", " · "),
        ("#9a927e", "w "),
    ] {
        assert!(
            card_run(&recorded, colour, value),
            "{value:?} in {colour}: {recorded:?}"
        );
    }
    assert!(recorded.iter().any(|line| line.contains("108w 72d ^")));
}

#[test]
fn pop_card_bars_cover_ratios_clamps_and_empty_jobs() {
    let recorded = notes(&cards_rendered_fixture());
    for (x, y, width) in [(58, 60, 48), (246, 60, 70), (70, 84, 20), (94, 84, 12)] {
        assert!(
            card_quad_at(&recorded, x, y, width, "#ece3cb"),
            "fill at ({x},{y}) width {width}: {recorded:?}"
        );
    }
    assert!(
        card_quad_at(&recorded, 58, 188, 80, "#ece3cb"),
        "300 days clamps to 40px"
    );
    assert!(
        card_quad_at(&recorded, 70, 212, 20, "#ece3cb"),
        "1200 needs clamps to 10px"
    );
    assert!(
        card_quad_at(&recorded, 246, 188, 80, "#3a3326"),
        "zero workers leaves only the job track"
    );
    assert!(
        !card_quad_at(&recorded, 246, 188, 0, "#ece3cb"),
        "zero job emits no fill quad"
    );
    assert!(card_text_at(&recorded, 146, 178, "300d"));
}

#[test]
fn building_cards_render_mockup_values_and_empty_or_clamped_bars() {
    let rig = cards_rendered_fixture_at(1280, 800, 2);
    let recorded = notes(&rig);
    assert_eq!(
        card_stacks(&rig).get(1).map(String::as_str),
        Some("gap=3 heights=50,39,39,39,39")
    );
    for text in [
        "mixed_farming 8, coastal_subsistence 1",
        "wage 3.2",
        "housing 3",
        "80/88",
    ] {
        assert!(
            recorded.iter().any(|line| line.contains(text)),
            "missing {text}: {recorded:?}"
        );
    }
    for (colour, value) in [
        ("#92ddea", "farm"),
        ("#92ddea", "wage "),
        ("#92ddea", "mixed_farming"),
        ("#5fb35a", "3.2"),
        ("#5fb35a", "8"),
        ("#5fb35a", "27"),
        ("#5fb35a", "36"),
        ("#9a927e", "/"),
        ("#92ddea", "positions"),
        ("#92ddea", "storage"),
    ] {
        assert!(
            card_run(&recorded, colour, value),
            "{value:?} in {colour}: {recorded:?}"
        );
    }
    assert!(card_text_at(&recorded, 32, 74, "27/36"));
    assert!(recorded.iter().any(|line| {
        line.starts_with("goodsquad ui/goods/cattle.png ") && line.ends_with(" 18 18 -")
    }));
    assert!(
        card_quad_at(&recorded, 116, 60, 42, "#ece3cb"),
        "the positions bar uses the approved 24px width and fills 21px"
    );
    assert!(
        card_quad_at(&recorded, 346, 60, 44, "#ece3cb"),
        "the storage bar fills proportionally within the remaining width: {recorded:?}"
    );
    assert!(
        card_quad_at(&recorded, 116, 166, 48, "#3a3326"),
        "empty positions track"
    );
    assert!(
        card_quad_at(&recorded, 322, 166, 80, "#3a3326"),
        "zero-capacity storage track: {recorded:?}"
    );
    assert!(card_run(&recorded, "#92ddea", "storage"));
    assert!(
        card_quad_at(&recorded, 322, 334, 80, "#ece3cb"),
        "over-capacity storage fill clamps to its track"
    );
    assert!(
        card_quad_at(&recorded, 322, 418, 80, "#3a3326"),
        "zero of 8000 draws the track"
    );
    assert!(
        !card_quad_at(&recorded, 290, 418, 0, "#ece3cb"),
        "zero fill creates no quad"
    );
}

#[test]
fn card_content_is_clipped_without_wrapping_long_place_values() {
    let mut rig = Rig::new_fixture_body(&script(&[
        "time(1, 1) select_place('fada', 'Fada')",
        "announce_tabs()",
    ]));
    // 300 points leaves a narrow card; long values are clipped by the group's viewport and do not wrap.
    rig.deliver_fixture_frame_at(300, 600, 2);
    rig.opened();
    rig.opened();
    click_tab_at(&mut rig, "pops", (300, 600));
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=61,61")
    );
}

// ---------------------------------------------------------------------------------------------
// The pops and buildings bars (CK-296, slice L3)
// ---------------------------------------------------------------------------------------------

#[test]
fn pop_cards_filter_by_type_with_a_count_the_same_items_show_as_rows_and_a_tick_nothing_passes_shows_no_card()
 {
    let mut rig = panel_rig(&[
        "time(1, 1) select_place('cards', 'Cards') announce_tabs()",
        "announce_panel_bar('cards') note_kinds('all_cards')",
        "announce_ppicker()",
        "announce_panel_bar('cards') note_kinds('monk_cards') announce_look()",
        "announce_panel_bar('cards') note_kinds('monk_rows')",
        "time(2, 2) announce_panel_bar('cards') note_kinds('no_monks_rows') announce_look()",
        "announce_panel_bar('cards') note_kinds('no_monks_cards') announce_tabs()",
        "select_place('nobody', 'No Economy') announce_panel_bar('cards') announce_look()",
    ]);
    rig.opened();
    click_tab(&mut rig, "pops");
    rig.opened();
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=61,61,61,61,61")
    );
    assert_eq!(
        pnote(&rig, "all_cards "),
        "all_cards peasant@1,monk@2,peasant@3,noble@4,monk@5"
    );
    assert_eq!(
        pnote(&rig, "pcount "),
        "pcount ",
        "no count with nothing ticked"
    );
    pclick(&mut rig, "pbar_filter");
    rig.opened();
    assert_eq!(pnote(&rig, "ppicker "), "ppicker filter monk,noble,peasant");
    ppick_click(&mut rig, "monk");
    rig.opened();
    assert_eq!(pnote(&rig, "monk_cards "), "monk_cards monk@2,monk@5");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=61,61")
    );
    assert_eq!(pnote(&rig, "pcount "), "pcount 2 of 5");
    // The look changes to rows: the same items, in the same order, with the same choices and the same bar.
    click_look(&mut rig, "rows");
    rig.opened();
    assert_eq!(pnote(&rig, "monk_rows "), "monk_rows monk@2,monk@5");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13,13,13"),
        "the header and the two rows"
    );
    assert_eq!(pnote(&rig, "pcount "), "pcount 2 of 5");
    assert!(
        pnote(&rig, "pstate ").starts_with("pstate ticks=1 sort=nil bars=2 "),
        "{}",
        pnote(&rig, "pstate ")
    );
    // A new revision with no monks: the tick stays and no item passes. Rows: only the header; cards: no card.
    rig.opened();
    assert_eq!(pnote(&rig, "pcount "), "pcount 0 of 2");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=1 heights=13"),
        "only the header"
    );
    click_look(&mut rig, "cards");
    rig.opened();
    assert_eq!(pnote(&rig, "pcount "), "pcount 0 of 2");
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights="),
        "no card, and not the `no pops` line"
    );
    // A place with no pops still says so, with no count.
    rig.opened();
    assert_eq!(
        card_stacks(&rig).last().map(String::as_str),
        Some("gap=3 heights=13")
    );
    assert_eq!(pnote(&rig, "pcount "), "pcount ");
}

#[test]
fn building_cards_order_by_positions_and_the_wage_hover_names_the_original_index() {
    let mut rig = panel_rig(&[
        "time(1, 1) select_place('cards', 'Cards') announce_tabs()",
        "announce_panel_bar('cards') note_kinds('all_buildings')",
        "announce_ppicker()",
        r"note_kinds('by_positions')
local P = require('lib.ui').P
local width = card_object.width // P
local wage_text = 'wage 3'
local wage_left = width - 5 - font.layout(wage_text).width
card_object.items[1].tooltip(wage_left + font.layout('wage ').width + 1, 4)
note('wage_hover ' .. last_content.class .. ' ' .. last_content.args.place .. ' ' .. last_content.args.building)",
    ]);
    rig.opened();
    click_tab(&mut rig, "buildings");
    rig.opened();
    assert_eq!(
        pnote(&rig, "all_buildings "),
        "all_buildings farm@1,holdings@2,storehouse@3,holdings@4,granary@5"
    );
    pclick(&mut rig, "pbar_sort");
    rig.opened();
    assert_eq!(
        pnote(&rig, "ppicker "),
        "ppicker sort default,kind,positions,wage"
    );
    ppick_click(&mut rig, "positions");
    rig.opened();
    assert_eq!(
        pnote(&rig, "by_positions "),
        "by_positions holdings@4,farm@1,holdings@2,storehouse@3,granary@5",
        "most positions filled first; equal ones in the given order"
    );
    assert_eq!(pnote(&rig, "wage_hover "), "wage_hover wage cards 4");
}
