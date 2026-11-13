use super::*;

#[test]
fn the_time_bar_and_location_tab_split_terms_numbers_and_ordinary_punctuation() {
    let mut rig = Rig::new_fixture_body(&script_with_prelude(
        "colour_watch = true",
        &["time(1, 1) select_place('rowan', 'Rowan')"],
    ));
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        tint_line(&recorded, "1 Jan 768 · 01:00", "#5fb35a", "1"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "1 Jan 768 · 01:00", "#ece3cb", " Jan "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "1 Jan 768 · 01:00", "#ece3cb", " · "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "people: ", "#92ddea", "people: "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "120", "#5fb35a", "120"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "basic ", "#92ddea", "basic "),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "80/100", "#5fb35a", "80"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "80/100", "#ece3cb", "/"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "215 days", "#5fb35a", "215"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "215 days", "#ece3cb", " days"),
        "{recorded:?}"
    );
    assert!(
        tint_line(&recorded, "4200 / 24200 m²", "#ece3cb", " m²"),
        "{recorded:?}"
    );
}

#[test]
fn market_table_uses_the_spec_columns_colours_and_cell_specific_sources() {
    let mut rig = rig_at(
        &[r"VIEWS[1].rowan.prices = {
  price('oats', 96500, 100, 820, 610), price('peas_beans', 1200, 2, 0, 140),
  price('seabirds', 750, 1, 510, 505), price('moonstone', 4500, 4),
  price('peas_beans', 123456789000, 8)
}; time(1, 1); select_place('rowan', 'Rowan')
local line = list_object
for _, item in ipairs(line.items) do
  if item.cells and item.cells[2] and item.cells[2].text == '123456789' then
    note('clipped_name=' .. item.cells[1].text)
    note('clipped_name_fits=' .. tostring(font.layout(item.cells[1].text).width <= 124 - font.layout(item.cells[2].text).width - 4 - 16))
  end
end
local function x_for(value) return line.origin_x + line.x + value end
local function row_index(label)
  for index, item in ipairs(line.items) do
    if item.cells ~= nil and item.cells[1] ~= nil and item.cells[1].text == label then return index end
  end
end
local function y_for(row) return line.origin_y + line.y - line.s + row * 26 - 13 end
local header, oats = row_index('good'), row_index('oats')
note_tooltip('header_good', line.row_keys[header], x_for(16 * P + 1), y_for(header))
note_tooltip('header_price', line.row_keys[header], x_for(124 * P - 1), y_for(header))
note_tooltip('header_buy', line.row_keys[header], x_for(158 * P), y_for(header))
note_tooltip('oats_icon', line.row_keys[oats], x_for(8), y_for(oats))
note_tooltip('oats_name', line.row_keys[oats], x_for(16 * P + 1), y_for(oats))
note_tooltip('oats_price', line.row_keys[oats], x_for(124 * P - 1), y_for(oats))
note_tooltip('oats_base', line.row_keys[oats], x_for(150 * P - 1), y_for(oats))
note_tooltip('oats_bar', line.row_keys[oats], x_for(158 * P), y_for(oats))
note_tooltip('oats_gap', line.row_keys[oats], x_for(158 * P - 1), y_for(oats))"],
        1274,
        800,
        2,
    );
    rig.opened();
    let recorded = notes(&rig);
    let price_rows = rows(&rig).last().expect("location rows").clone();
    assert!(price_rows.contains("good@16:left:#92ddea,price@124:right:#92ddea,base@150:right:#92ddea,buy/sell@158:left:#9a927e"), "header columns and term/dim colours: {price_rows}");
    assert!(price_rows.contains("oats@16:left:#92ddea,96.5@124:right:-,100@150:right:-;stripe=-;icon=goods/oats;bar=158,46,"), "icon, text and numeric columns: {price_rows}");
    assert!(price_rows.contains("peas and..@16:left:#92ddea,1.2@124:right:-,2@150:right:-;stripe=-;icon=goods/peas_beans;bar=158,46,"), "good titles are used in the table and clipped when the widest price needs room: {price_rows}");
    assert!(
        price_rows.contains("moonsto..@16:left:#ece3cb,4.5@124:right:-,4@150:right:-;stripe=-;icon=goods/fallback/unknown;bar=158,46,nil"),
        "a good without its own icon is now a market row with the fallback art: {price_rows}"
    );
    assert!(
        !price_rows.contains("  moonstone 4.5 (4)"),
        "the old plain line is gone: {price_rows}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line.starts_with("clipped_name=") && line.ends_with("..")),
        "the name is clipped with an ellipsis: {recorded:?}"
    );
    assert!(
        recorded.iter().any(|line| line == "clipped_name_fits=true"),
        "the clipped name ends at least four pixels before the widest price: {recorded:?}"
    );
    assert!(
        !recorded
            .iter()
            .any(|line| line.contains("ui/goods/moonstone.png")),
        "a good without an icon does not attempt a texture load: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line.starts_with("goodsquad ui/goods/oats.png 8 ")
                && line.ends_with(" 18 18 -")),
        "the existing good icon is retained: {recorded:?}"
    );
    for expected in [
        "tooltip header_good title=goods|Things that are made, kept, traded |and used up. Each good has a price |where it is traded, and meets some |of a pop's needs. part=1",
        "tooltip header_price title=price|What a goods is worth at a location |now. It rises when more is wanted |than offered, and falls when more |is offered than wanted. part=2",
        "tooltip header_buy  part=-",
        "tooltip oats_icon title=oats|A grain, eaten as porridge and |bread. part=icon",
        "tooltip oats_name title=oats|A grain, eaten as porridge and |bread. part=1",
        "tooltip oats_price oats: price 96.5|base price 100|bought (buy orders) 820|offered (sell orders) 610 part=2",
        "tooltip oats_base title=base price|The usual worth of a goods, around |which its price moves. part=3",
        "tooltip oats_bar oats: price 96.5|base price 100|bought (buy orders) 820|offered (sell orders) 610 part=bar",
        "tooltip oats_gap  part=-",
    ] {
        assert!(
            recorded.iter().any(|line| line == expected),
            "{expected}: {recorded:?}"
        );
    }
}

#[test]
fn unlisted_goods_are_market_rows_with_a_cached_unknown_icon_and_no_explanation() {
    let mut rig = rig_at(
        &[r"VIEWS[1].rowan.prices = {
  price('oats', 100000, 100), price('moonstone', 4500, 4), price('quartz', 2500, 3)
}; time(1, 1); select_place('rowan', 'Rowan')
local line = list_object
local function row_index(label)
  for index, item in ipairs(line.items) do
    if item.cells and item.cells[1] and item.cells[1].text == label then return index end
  end
end
local function x_for(value) return line.origin_x + line.x + value end
local function y_for(row) return line.origin_y + line.y - line.s + row * 26 - 13 end
for _, name in ipairs({'moonstone', 'quartz'}) do
  local index = row_index(name)
  local key = line.row_keys[index]
  note_tooltip(name .. '_icon', key, x_for(8), y_for(index))
  note_tooltip(name .. '_name', key, x_for(17 * P), y_for(index))
  note_tooltip(name .. '_price', key, x_for(124 * P - 1), y_for(index))
end"],
        1274,
        800,
        2,
    );
    rig.opened();
    let recorded = notes(&rig);
    let price_rows = rows(&rig).pop().expect("market rows");
    for (name, price, base) in [("moonstone", "4.5", "4"), ("quartz", "2.5", "3")] {
        assert!(
            price_rows.contains(&format!(
                "{name}@16:left:#ece3cb,{price}@124:right:-,{base}@150:right:-;stripe=-;icon=goods/fallback/unknown;bar=158,46,nil"
            )),
            "an unlisted good without a glossary entry uses the text colour: {price_rows}"
        );
        assert!(
            recorded
                .iter()
                .any(|line| line == &format!("tooltip {name}_icon  part=-")),
            "an unlisted icon without a glossary entry is not hoverable: {recorded:?}"
        );
        assert!(
            recorded
                .iter()
                .any(|line| line == &format!("tooltip {name}_name  part=-")),
            "an unlisted name without a glossary entry is not hoverable: {recorded:?}"
        );
        assert!(
            recorded
                .iter()
                .any(|line| line.starts_with(&format!("tooltip {name}_price {name}: price "))),
            "the price remains hoverable for an unlisted good: {recorded:?}"
        );
        assert!(
            !recorded
                .iter()
                .any(|line| line.contains(&format!("ui/goods/{name}.png"))),
            "no per-good texture is loaded for an unlisted good: {recorded:?}"
        );
    }
    assert!(
        !price_rows.contains("  moonstone 4.5 (4)"),
        "the CK-282 plain-line fallback is replaced: {price_rows}"
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|line| line.as_str() == "goodsload ui/goods/fallback/unknown.png")
            .count(),
        1,
        "two unlisted goods share the cached fallback texture: {recorded:?}"
    );
}

#[test]
fn an_unlisted_good_with_a_glossary_entry_uses_the_term_title_and_tooltip() {
    let mut rig = rig_with_prelude_at(
        r"local real_entry = glossary.entry
glossary.entry = function(key)
  if key == 'moonstone' then return {key = key, title = 'moon stone', text = 'A test mineral.'} end
  return real_entry(key)
end",
        &[r"VIEWS[1].rowan.prices = {price('moonstone', 4500, 4)}
time(1, 1); select_place('rowan', 'Rowan')
local line = list_object
local index
for row, item in ipairs(line.items) do
  if item.cells and item.cells[1] and item.cells[1].text == 'moon stone' then index = row end
end
assert(index ~= nil, 'the glossary title is a market row')
local key = line.row_keys[index]
local y = line.origin_y + line.y - line.s + index * 26 - 13
note_tooltip('moonstone_icon', key, line.origin_x + line.x + 8, y)
note_tooltip('moonstone_name', key, line.origin_x + line.x + 34, y)"],
        1274,
        800,
        2,
    );
    rig.opened();
    let recorded = notes(&rig);
    let market = rows(&rig).last().unwrap().clone();
    assert!(
        market.contains("moon stone@16:left:#92ddea")
            && market.contains("icon=goods/fallback/unknown"),
        "the glossary title uses term colour independently of the fallback icon: {market}"
    );
    for expected in [
        "tooltip moonstone_icon title=moon stone|A test mineral. part=icon",
        "tooltip moonstone_name title=moon stone|A test mineral. part=1",
    ] {
        assert!(
            recorded.iter().any(|line| line == expected),
            "missing {expected}: {recorded:?}"
        );
    }
    assert!(
        !recorded
            .iter()
            .any(|line| line.contains("ui/goods/moonstone.png")),
        "the glossary entry does not change icon selection: {recorded:?}"
    );
}

#[test]
fn price_cell_tooltip_uses_the_price_class_and_its_breakdown_refreshes() {
    let mut rig = rig(&[
        r#"VIEWS[1].rowan.prices = {price("oats", 96500, 100)}; time(1, 1); select_place("rowan", "Rowan")
local function oats_row_key()
  for index, item in ipairs(list_object.items) do
    if item.cells and item.cells[1] and item.cells[1].text == 'oats' then return list_object.row_keys[index], index end
  end
end
local oats_key, oats_index = oats_row_key()
local oats_x = list_object.origin_x + list_object.x + 200 * P
local oats_y = list_object.origin_y + list_object.y - list_object.s + (oats_index - 1) * lists.ROW + 10
note_tooltip("oats", oats_key, oats_x, oats_y); note_tooltip_kind("price_oats", oats_key)
note_price_content("empty", "rowan", "oats")
VIEWS[2] = VIEWS[1]; VIEWS[2].rowan.prices = {price("oats", 97250, 100, 1240, 1410)}; time(2, 2)
note_price_content("updated", "rowan", "oats")"#,
    ]);
    rig.opened();
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip oats oats: price 96.5|base price 100|bought (buy orders) -|offered (sell orders) - part=2"),
        "row tooltip: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|line| line == "tooltip_kind price_oats lockable"),
        "price rows remain lockable: {recorded:?}"
    );
    assert!(recorded.iter().any(|line| line == "price_content empty oats: price 96.5|base price 100|bought (buy orders) -|offered (sell orders) -"), "empty breakdown: {recorded:?}");
    assert!(recorded.iter().any(|line| line == "price_content updated oats: price 97.25|base price 100|bought (buy orders) 1240|offered (sell orders) 1410"), "refreshed breakdown: {recorded:?}");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the real price-row witness verifies tooltip delay, passive input, row removal and relayout in one UI lifetime"
)]
fn real_price_row_hover_is_passive_survives_resize_and_leaves_no_old_keys() {
    let mut rig = rig(&[
        r"VIEWS[1].rowan.prices = {price('oats', 96500, 100), price('moonstone', 4500, 4)}
time(1, 1); select_place('rowan', 'Rowan')
local text = require('lib.text'); local real_set = text.set
text.set = function(line, value) real_set(line, value); if capture_tips then note('tooltip_text ' .. value) end end
capture_tips = true
frame_test_observer = function()
  note('list_hover ' .. tostring(list_object.hover_row) .. ' pressed ' .. tostring(list_object.pressed_row))
end
local function oats_row_index()
  for index, item in ipairs(list_object.items) do
    if item.cells and item.cells[1] and item.cells[1].text == 'oats' then return index end
  end
end
old_price_key = list_object.row_keys[oats_row_index()]
local x = list_object.origin_x + list_object.x + 212 * P - 1
local y = list_object.origin_y + list_object.y + (oats_row_index() - 1) * lists.ROW + 10
note(string.format('oats_point %g %g', x, y)); note('old_price_key ' .. old_price_key)",
        r"local function resized_oats_row_index()
  for index, item in ipairs(list_object.items) do
    if item.cells and item.cells[1] and item.cells[1].text == 'oats' then return index end
  end
end
local x = list_object.origin_x + list_object.x + 212 * P - 1
local y = list_object.origin_y + list_object.y + (resized_oats_row_index() - 1) * lists.ROW + 10
note('old_key_live ' .. tostring(require('lib.pointer').widget(old_price_key) ~= nil))
note(string.format('resized_oats_point %g %g', x, y)); note('resized_price_key ' .. list_object.row_keys[resized_oats_row_index()])",
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 1);
    rig.opened();

    let recorded = notes(&rig);
    let point = recorded
        .iter()
        .find(|line| line.starts_with("oats_point "))
        .unwrap();
    let mut coords = point
        .split_whitespace()
        .skip(1)
        .map(|value| value.parse::<f64>().unwrap());
    let price_point = (coords.next().unwrap(), coords.next().unwrap());
    let old_price_key = recorded
        .iter()
        .find_map(|line| line.strip_prefix("old_price_key "))
        .unwrap();
    assert_eq!(
        rig.key_at(price_point.0, price_point.1).as_deref(),
        Some(old_price_key)
    );
    rig.feed(&[PointerEvent::Move(points(price_point.0, price_point.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 1);
    }
    let opened = notes(&rig);
    let spans = [
        "tooltip_text oats: price ",
        "tooltip_text 96.5",
        "tooltip_text base price ",
        "tooltip_text 100",
    ];
    assert!(
        opened.windows(spans.len()).any(|window| window == spans),
        "price tooltip did not open the class breakdown: {opened:?}"
    );

    rig.feed(&[
        PointerEvent::Down(points(price_point.0, price_point.1)),
        PointerEvent::Up(points(price_point.0, price_point.1)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 1);
    let recorded = notes(&rig);
    assert!(
        recorded
            .iter()
            .any(|line| line == "list_hover nil pressed nil")
    );

    // The text-only moonstone line has a passive key in this tooltip-enabled list but no tooltip source.
    let moonstone_y = price_point.1 + 3.0 * 26.0;
    let before = notes(&rig).len();
    rig.feed(&[PointerEvent::Move(points(price_point.0, moonstone_y))]);
    rig.deliver_fixture_frame_at(1800, 900, 1);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1800, 900, 1);
    }
    let after = notes(&rig);
    assert!(
        !after[before..]
            .iter()
            .any(|line| line.starts_with("tooltip_text ")),
        "the unlisted good line has no tooltip: {:?}",
        &after[before..]
    );

    rig.deliver_fixture_frame_at(1600, 900, 1);
    rig.opened();
    let resized = notes(&rig);
    assert!(resized.iter().any(|line| line == "old_key_live false"));
    let point = resized
        .iter()
        .find(|line| line.starts_with("resized_oats_point "))
        .unwrap();
    let mut coords = point
        .split_whitespace()
        .skip(1)
        .map(|value| value.parse::<f64>().unwrap());
    let resized_point = (coords.next().unwrap(), coords.next().unwrap());
    rig.feed(&[PointerEvent::Move(points(resized_point.0, resized_point.1))]);
    for _ in 0..22 {
        rig.deliver_fixture_frame_at(1600, 900, 1);
    }
    let final_notes = notes(&rig);
    assert!(
        final_notes
            .windows(spans.len())
            .any(|window| window == spans),
        "resized price tooltip did not preserve the class breakdown: {final_notes:?}"
    );
}
