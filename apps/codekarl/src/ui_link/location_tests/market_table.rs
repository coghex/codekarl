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

// ---------------------------------------------------------------------------------------------
// The market's list bar (CK-296, slice L3)
// ---------------------------------------------------------------------------------------------

/// Lua for the market tests: the lines of figures and the header (everything before the first goods row) as one text.
const MARKET_HELPERS: &str = r#"
function figures()
  local texts = {}
  for _, item in ipairs(list_object.items) do
    if item.bar ~= nil then break end
    local cells = {}
    for _, cell in ipairs(item.cells) do cells[#cells + 1] = cell.text end
    texts[#texts + 1] = table.concat(cells, "|")
  end
  return table.concat(texts, "/")
end
-- The list item of the first goods row.
function first_goods_item()
  for _, item in ipairs(list_object.items) do
    if item.bar ~= nil then return item end
  end
end
"#;

fn shipped_panel_rig(extra: &str, steps: &[&str]) -> Rig {
    let (actual_place, _) = ledger::shipped_place_fixture();
    let prelude = format!("{PANEL_BARS}\n{MARKET_HELPERS}\n{extra}");
    let script = script_with_prelude(&prelude, steps);
    let mut rig =
        Rig::new_fixture_body_with_functions(&script, vec![("actual_place", actual_place)]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig
}

fn numbers_of(line: &str) -> Vec<i64> {
    line.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|part| !part.is_empty() && *part != "-")
        .map(|part| part.parse().unwrap())
        .collect()
}

#[test]
fn the_location_list_is_shorter_by_the_bar_and_the_market_filters_by_tag_on_the_shipped_goods() {
    let mut rig = shipped_panel_rig(
        "",
        &[
            "time(1, 1) select_place('clonmacnoise', 'Cluain Mhic Nóis')",
            r"announce_panel_bar('list')
note('plist rows=' .. list_object.rows .. ' view=' .. list_object.view_height)
figures_before = figures()
note('goods ' .. #panel_given)
local untagged = {}
for _, row in ipairs(panel_given) do if #row.filter.tag == 0 then untagged[#untagged + 1] = row.sort.name end end
note('untagged ' .. table.concat(untagged, ','))",
            "announce_ppicker()",
            "note_order('herd_order') announce_panel_bar('list') note('figures_same ' .. tostring(figures() == figures_before))",
            r"local missing = {}
for _, tag in ipairs({'clothing', 'food', 'herd'}) do
  panel_bar:clear()
  panel_bar:toggle(1, tag)
  for _, row in ipairs(panel_shown) do
    if row.sort.name == 'herbs' or row.sort.name == 'beeswax' then missing[#missing + 1] = tag end
  end
end
note('untagged_in_filters ' .. #missing)
panel_bar:clear()
note('after_all ' .. #panel_shown)",
            "announce_ppicker()",
        ],
    );
    rig.opened();
    rig.opened();
    // The list is whole rows and ends above the bar; the buttons are under it at the content's right.
    let content = numbers_of(&pnote(&rig, "pbar_content "));
    let (width, height) = (content[0], content[1]);
    let list = numbers_of(&pnote(&rig, "plist "));
    assert_eq!(list[1], list[0] * 26);
    assert_eq!(list[0], (height - 38) / 26, "{content:?} {list:?}");
    assert_eq!(
        pnote(&rig, "pbar_sort_rect "),
        format!("pbar_sort_rect {} {} 26 30", width - 26, height - 34)
    );
    assert_eq!(
        pnote(&rig, "pbar_filter_rect "),
        format!("pbar_filter_rect {} {} 26 30", width - 58, height - 34)
    );
    assert_eq!(pnote(&rig, "goods "), "goods 22");
    assert_eq!(pnote(&rig, "untagged "), "untagged herbs,beeswax");
    assert_eq!(
        pnote(&rig, "pcount "),
        "pcount ",
        "no count with nothing ticked"
    );
    pclick(&mut rig, "pbar_filter");
    rig.opened();
    assert_eq!(
        pnote(&rig, "ppicker "),
        "ppicker filter clothing,food,herd",
        "the tags the goods carry, with no heading"
    );
    ppick_click(&mut rig, "herd");
    rig.opened();
    assert_eq!(pnote(&rig, "herd_order "), "herd_order cattle,pigs,sheep");
    assert_eq!(pnote(&rig, "pcount "), "pcount 3 of 22");
    assert_eq!(pnote(&rig, "figures_same "), "figures_same true");
    rig.opened();
    assert_eq!(pnote(&rig, "untagged_in_filters "), "untagged_in_filters 0");
    assert_eq!(pnote(&rig, "after_all "), "after_all 22");
    pclick(&mut rig, "pbar_sort");
    rig.opened();
    assert_eq!(
        pnote(&rig, "ppicker "),
        "ppicker sort default,name,price,base price,buy/sell"
    );
}

#[test]
fn the_market_sorts_by_price_buy_sell_and_name_with_the_missing_buy_sell_last_in_both_directions() {
    let extra = "VIEWS[1].rowan.prices = {
  price('oats', 96500, 100, 820, 610), price('peas_beans', 1200, 2, 0, 140),
  price('seabirds', 750, 1, 510, 505), price('moonstone', 4500, 4),
}";
    let mut rig = rig_with_prelude_at(
        &format!("{PANEL_BARS}\n{MARKET_HELPERS}\n{extra}"),
        &[
            "time(1, 1) select_place('rowan', 'Rowan') announce_panel_bar('list')",
            "announce_ppicker()",
            // price: highest first; the first row's price cell asks for that good's `price` tooltip.
            "note_order('by_price') first_goods_item().tooltip(0, 0, 2) note('hover ' .. last_content.class .. ' ' .. last_content.args.good)",
            "note_order('by_price_again')",
            "note_order('by_buy_sell')",
            "note_order('by_buy_sell_again')",
            "note_order('by_name')",
        ],
        1800,
        900,
        2,
    );
    rig.opened();
    pclick(&mut rig, "pbar_sort");
    rig.opened();
    ppick_click(&mut rig, "price");
    rig.opened();
    assert_eq!(
        pnote(&rig, "by_price "),
        "by_price oats,moonstone,peas and beans,seabirds"
    );
    assert_eq!(pnote(&rig, "hover "), "hover price oats");
    ppick_click(&mut rig, "price");
    rig.opened();
    assert_eq!(
        pnote(&rig, "by_price_again "),
        "by_price_again seabirds,peas and beans,moonstone,oats",
        "pressed again: lowest first"
    );
    ppick_click(&mut rig, "buy/sell");
    rig.opened();
    assert_eq!(
        pnote(&rig, "by_buy_sell "),
        "by_buy_sell peas and beans,seabirds,oats,moonstone",
        "largest surplus first, the good without a value last"
    );
    ppick_click(&mut rig, "buy/sell");
    rig.opened();
    assert_eq!(
        pnote(&rig, "by_buy_sell_again "),
        "by_buy_sell_again oats,seabirds,peas and beans,moonstone",
        "the other direction, the good without a value still last"
    );
    ppick_click(&mut rig, "name");
    rig.opened();
    assert_eq!(
        pnote(&rig, "by_name "),
        "by_name moonstone,oats,peas and beans,seabirds",
        "'peas and beans' sorts under p, by the name shown"
    );
}

/// What the location tab's layout was asked for: the widths `location.lines` and `location.rows` were called with, in order.
const WIDTH_SPY: &str = r#"
spy_widths = {}
do
  local location_library = require("lib.location")
  local real_lines, real_rows = location_library.lines, location_library.rows
  location_library.lines = function(key, place, width) spy_widths[#spy_widths + 1] = "lines " .. width; return real_lines(key, place, width) end
  location_library.rows = function(lines, width) spy_widths[#spy_widths + 1] = "rows " .. width; return real_rows(lines, width) end
end
function market_columns()
  local columns = {}
  for _, item in ipairs(list_object.items) do
    if item.cells ~= nil and item.cells[1] ~= nil and (item.cells[1].text == "good" or item.cells[1].text:match("^good")) and item.cells[2] ~= nil then
      columns[#columns + 1] = item
    end
  end
  return columns
end
"#;

#[test]
fn a_market_with_more_goods_than_fit_lays_its_columns_out_against_the_row_width_and_a_tag_filter_crosses_the_boundary_both_ways()
 {
    let mut rig = rig_with_prelude_at(
        &format!("{PANEL_BARS}\n{WIDTH_SPY}"),
        &[
            r"local goods = {}
for i = 1, 60 do
  local entry = price('good' .. i, 1000 + i * 10, 5 + i % 7, 10, 20)
  entry.tags = {i <= 3 and 'rare' or 'common'}
  goods[i] = entry
end
VIEWS[1].rowan.prices = goods
time(1, 1); select_place('rowan', 'Rowan')
local px = panel_bar.window.content_width // P
-- 60 goods and the figures are more rows than the list holds: its bar shows, and its rows are 5 pixels narrower.
expect_that(#list_object.items * 26 > list_object.view_height, 'more rows than fit')
local at = 1
while list_object.items[at].cells[1].text ~= 'good' do at = at + 1 end
local header = list_object.items[at]
eq_that(header.cells[1].text, 'good', 'the header')
eq_that(header.cells[2].x, px - 5 - 82, 'prices end at the row width less 82')
eq_that(header.cells[3].x, px - 5 - 56, 'base prices end at the row width less 56')
eq_that(header.cells[4].x, px - 5 - 48, 'the balance bar runs from the row width less 48')
local row = list_object.items[at + 1]
eq_that(row.cells[2].x, px - 5 - 82) eq_that(row.cells[3].x, px - 5 - 56)
eq_that(row.bar.x, px - 5 - 48) eq_that(row.bar.width, 46, 'the balance bar stays whole')
expect_that(row.bar.x + row.bar.width <= px - 5, 'and clear of the gap and the track')
local widest = 0
for _, item in ipairs(list_object.items) do
  if item.bar ~= nil then
    widest = math.max(widest, font.layout(item.cells[2].text).width)
    expect_that(font.layout(item.cells[1].text).width + 16 + widest + 4 <= px - 5 - 82 + widest, 'the name leaves its gap before the widest price')
  end
end
-- The lines of figures were wrapped for the row width: the layout was asked for the full width and then for the row width.
eq_that(spy_widths[#spy_widths - 3], 'lines ' .. (px * 2), 'the full width first')
eq_that(spy_widths[#spy_widths - 2], 'rows ' .. (px * 2), 'wrapped at the full width first')
eq_that(spy_widths[#spy_widths - 1], 'lines ' .. ((px - 5) * 2), 'then the figures and columns for the row width')
eq_that(spy_widths[#spy_widths], 'rows ' .. ((px - 5) * 2), 'and wrapped at the row width')
note('tab ready')",
            r"panel_bar:toggle(1, 'rare')
local px = panel_bar.window.content_width // P
expect_that(#list_object.items * 26 <= list_object.view_height, 'three goods fit')
local at = 1
while list_object.items[at].cells[1].text ~= 'good' do at = at + 1 end
local header = list_object.items[at]
eq_that(header.cells[2].x, px - 82, 'the full width again')
eq_that(header.cells[3].x, px - 56) eq_that(header.cells[4].x, px - 48)
eq_that(list_object.items[at + 1].bar.x, px - 48)
eq_that(spy_widths[#spy_widths], 'rows ' .. (px * 2), 'wrapped at the full width')",
            r"panel_bar:clear()
local px = panel_bar.window.content_width // P
local at = 1
while list_object.items[at].cells[1].text ~= 'good' do at = at + 1 end
local header = list_object.items[at]
eq_that(header.cells[2].x, px - 5 - 82, 'clearing the filter gives the narrower columns back')
eq_that(list_object.items[at + 1].bar.x, px - 5 - 48)",
        ],
        1274,
        800,
        2,
    );
    rig.opened();
    rig.opened();
    rig.opened();
}

/// What `location.rows` returned for each layout of the tab: the width it was asked for and how many rows it made.
const ROWS_SPY: &str = r"
spy_rows = {}
do
  local location_library = require('lib.location')
  local real_rows = location_library.rows
  location_library.rows = function(lines, width)
    local made = real_rows(lines, width)
    spy_rows[#spy_rows + 1] = {width, #made}
    return made
  end
end
function note_wrap(label)
  local last, before = spy_rows[#spy_rows], spy_rows[#spy_rows - 1]
  local two_passes = before ~= nil and before[1] > last[1]
  note(string.format('%s items=%d rows=%d width=%d full_rows=%s', label, #list_object.items, last[2], last[1], two_passes and before[2] or 'none'))
end
const_goods = function()
  local goods = {}
  for i = 1, 60 do
    local entry = price('good' .. i, 1000 + i * 10, 5 + i % 7, 10, 20)
    entry.tags = {i <= 3 and 'rare' or 'common'}
    goods[i] = entry
  end
  VIEWS[1].rowan.prices = goods
end
";

fn wrap_note(rig: &Rig, label: &str) -> (i64, i64, i64, Option<i64>) {
    let line = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(&format!("{label} ")))
        .unwrap_or_else(|| panic!("no {label}: {:?}", notes(rig)));
    let field = |name: &str| {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name} in {line}"))
            .to_owned()
    };
    (
        field("items").parse().unwrap(),
        field("rows").parse().unwrap(),
        field("width").parse().unwrap(),
        field("full_rows").parse().ok(),
    )
}

#[test]
fn a_width_where_the_bar_forces_one_more_wrapped_line_lays_the_figures_out_again_and_a_fitting_filter_gives_the_full_layout_back()
 {
    // The panel is a third of the window: scan the window's width until the figures wrap into more rows at the row width than at the full
    // width (a line that fits the full width and not the 5 pixels less).
    let found = (1200..=1800).step_by(3).find_map(|width| {
        let mut rig = rig_with_prelude_at(
            &format!("{PANEL_BARS}\n{ROWS_SPY}"),
            &["const_goods() time(1, 1); select_place('rowan', 'Rowan') note_wrap('probe')"],
            width,
            800,
            2,
        );
        rig.opened();
        let (_, rows, _, full_rows) = wrap_note(&rig, "probe");
        full_rows
            .filter(|full| rows > *full)
            .map(|full| (width, full, rows))
    });
    let (width, full_rows, narrow_rows) =
        found.expect("some width wraps a figure line only at the row width");
    let mut rig = rig_with_prelude_at(
        &format!("{PANEL_BARS}\n{ROWS_SPY}"),
        &[
            "const_goods() time(1, 1); select_place('rowan', 'Rowan') note_wrap('overflowing')",
            "panel_bar:toggle(1, 'rare') note_wrap('fitting')",
            "panel_bar:clear() note_wrap('cleared')",
        ],
        width,
        800,
        2,
    );
    rig.opened();
    // Overflowing: laid out at the row width, with the extra wrapped row, and the list shows all of it.
    let (items, rows, row_width, full) = wrap_note(&rig, "overflowing");
    assert_eq!(
        (items, rows, full),
        (narrow_rows, narrow_rows, Some(full_rows))
    );
    assert!(narrow_rows > full_rows, "the bar made a line wrap");
    assert!(
        row_width < width.into(),
        "{row_width} is the row width, the narrower one"
    );
    rig.opened();
    // Fitting after a filter that leaves 3 goods: the full layout again, one row fewer per wrapped line, and nothing of the bar's width.
    let (items, _, fitting_width, _) = wrap_note(&rig, "fitting");
    assert_eq!(
        items,
        full_rows - 57,
        "60 goods less 57, laid out at the full width"
    );
    assert_eq!(
        fitting_width,
        row_width + 10,
        "the full width is 5 UI pixels (10 points) more"
    );
    rig.opened();
    // Cleared: overflowing again, with the same layout as before.
    let (items, rows, again_width, _) = wrap_note(&rig, "cleared");
    assert_eq!(
        (items, rows, again_width),
        (narrow_rows, narrow_rows, row_width)
    );
}
