//! The scroll bar (CK-299, `scripts/ui/lib/scrollbar.lua`) on the scroll list and the card stack, in the UI rig with no window: the
//! shipped libraries run over the real `render` API, a Lua shadow of the scene (the list bar tests' prelude) records every quad, group,
//! clip and hit region, and Rust feeds real pointer events through the mapper, the router and `pointer.dispatch`. Nothing sleeps.
//!
//! The picker's bar is in `listbar_tests.rs`; the callers' columns are in `location_tests/`.

use ck_window::{PointerEvent, ScrollUnit};

use super::group_tests::Rig;
use super::listbar_tests::{frame, hover, notes, point, rig};
use super::widget_witness_tests::points;

/// A window of 120 by 100 UI pixels (240 by 200 points) with a list of `count` strings, 240 wide and 5 rows (130) high, and the helpers
/// the tests share. The bar's quads are the track then the thumb, in the bar's own group.
const LIST: &str = r#"
selected = nil
function names(count)
  local t = {}
  for i = 1, count do t[i] = "Item " .. i end
  return t
end
function new_list(count)
  new_window(120, 100)
  L = lists.new{window = W, x = 0, y = 0, width = 120 * P, rows = 5, items = names(count),
    on_select = function(index) selected = index; note("select " .. index) end}
  return L
end
function parts(owner)
  local quads = quads_in(owner.bar.group)
  return quads[1], quads[2]
end
function thumb_rect(owner)
  local _, thumb = parts(owner)
  return quad_at(thumb)
end
function track_rect(owner)
  local track = parts(owner)
  return quad_at(track)
end
-- The thumb of a view over a taller content at a scroll, worked out again here from the rule.
function expected_thumb(view, content, scroll)
  local function snap(v) return math.floor(v / P + 0.5) * P end
  local length = math.min(view, math.max(9 * P, snap(view * view / content)))
  local travel = view - length
  local top = 0
  if travel > 0 then top = math.min(travel, snap(travel * scroll / (content - view))) end
  return length, top
end
function event(kind, key, x, y) return {kind = kind, key = key, x = x, y = y, inside = true} end
on_frame = function() if L ~= nil and not L.closed then note("s=" .. L:scroll_position()) end end
"#;

fn last(rig: &Rig, prefix: &str) -> String {
    let Some(line) = notes(rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with(prefix))
    else {
        panic!("no note {prefix}: {:?}", notes(rig));
    };
    line[prefix.len()..].to_owned()
}

fn press_move_release(rig: &mut Rig, from: (f64, f64), to: (f64, f64)) {
    rig.feed(&[
        PointerEvent::Down(points(from.0, from.1)),
        PointerEvent::Move(points(to.0, to.1)),
        PointerEvent::Up(points(to.0, to.1)),
    ]);
    frame(rig);
}

fn wheel(rig: &mut Rig, (x, y): (f64, f64), lines: i32) {
    rig.feed(&[PointerEvent::Scroll {
        position: points(x, y),
        amount: lines * 256,
        unit: ScrollUnit::Lines,
    }]);
    frame(rig);
}

// ---------------------------------------------------------------------------------------------------------------------
// When the bar is there
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_list_has_a_bar_only_while_its_items_are_taller_than_its_view_and_crosses_both_ways_by_a_change_of_items()
 {
    let mut rig = rig(
        LIST,
        &[r#"
new_list(5)
local ox, oy = content_origin()
eq(#quads_in(L.bar.group), 0, "5 items fit 5 rows: no quad")
eq(count_hits("scroll_"), 0, "and no hit region")
local x, y, w, h = hit_rect(L.row_keys[1])
eq(w, 240, "a row is as wide as the view while there is no bar")
L:set_items(names(6))
eq(#quads_in(L.bar.group), 2, "6 items overflow: a track and a thumb")
eq(count_hits("scroll_"), 2, "and their hit regions")
local tx, ty, tw, th = track_rect(L)
eq(tx, ox + 240 - 6) eq(ty, oy) eq(tw, 6, "3 UI pixels wide") eq(th, 130, "as tall as the view")
x, y, w, h = hit_rect(L.row_keys[1])
eq(w, 230, "a row is 5 UI pixels narrower")
L:set_items(names(5))
eq(#quads_in(L.bar.group), 0, "the bar goes with the overflow")
eq(count_hits("scroll_"), 0, "and its regions")
x, y, w, h = hit_rect(L.row_keys[1])
eq(w, 240, "and the rows are as wide as the view again")
L:set_items(names(40))
eq(#quads_in(L.bar.group), 2)
-- A scroll never changes whether the bar is there.
L:scroll_to(40)
eq(#quads_in(L.bar.group), 2)
"#],
    );
    rig.opened();
}

#[test]
fn the_row_width_a_list_gives_is_the_view_while_it_fits_and_5_pixels_less_while_it_does_not() {
    let mut rig = rig(
        LIST,
        &[r#"
eq(lists.row_width(240, 5, 5), 240, "a full view")
eq(lists.row_width(240, 5, 0), 240, "an empty list")
eq(lists.row_width(240, 5, 6), 230, "one row too many")
eq(lists.row_width(240, 5, 400), 230)
eq(lists.row_width(241, 5, 6), 232, "the width is snapped to the grid as the list snaps it: 242 less 10")
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------------------------------
// The thumb
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_thumb_is_the_shown_share_of_the_content_and_stands_where_the_scroll_puts_it() {
    let mut rig = rig(
        LIST,
        &[r#"
new_list(20)
local ox, oy = content_origin()
local x, y, w, h = thumb_rect(L)
eq(x, ox + 234) eq(w, 6)
eq(h, 32, "130 * 130 / 520 = 32.5, rounded to a pixel")
eq(y, oy, "at the start the thumb is at the track's top")
for _, scroll in ipairs({0, 26, 182, 364, 390}) do
  L:set_pos(scroll)
  local length, top = expected_thumb(130, 520, L:scroll_position())
  x, y, w, h = thumb_rect(L)
  eq(h, length, "length at " .. scroll) eq(y - oy, top, "top at " .. scroll)
end
eq(y - oy + h, 130, "at the end the thumb's bottom is the track's bottom")
-- A content so long that the share is under a pixel: the least length of 9 pixels.
L:set_items(names(1000))
L:set_pos(0)
x, y, w, h = thumb_rect(L)
eq(h, 18, "the least thumb is 9 UI pixels")
L:set_pos(25974)
x, y, w, h = thumb_rect(L)
eq(y - oy + h, 130, "and it still reaches the track's bottom")
-- The thumb is in the hit regions as it is drawn.
local hx, hy, hw, hh = hit_rect(L.bar.thumb_key)
eq(hx, x) eq(hy, y) eq(hw, w) eq(hh, h)
"#],
    );
    rig.opened();
}

#[test]
fn a_view_too_short_to_give_the_thumb_any_travel_has_a_thumb_that_fills_the_track_a_drag_does_nothing_and_the_wheel_scrolls()
 {
    let mut rig = rig(
        &format!(
            r#"{LIST}
function new_stack(height, count)
  new_window(120, 100)
  local items = {{}}
  for i = 1, count do items[i] = {{height = 13, draw = function() return {{destroy = function() end}} end}} end
  L = cards.new{{window = W, x = 0, y = 0, width = 120 * P, height = height, gap = 1, items = items}}
  return L
end
cards = require("lib.cards")
"#
        ),
        &[r#"
new_stack(5 * P, 10)
local ox, oy = content_origin()
local x, y, w, h = thumb_rect(L)
eq(h, 10, "a view 5 pixels high has a thumb 5 pixels long: never longer than the view")
eq(y, oy, "at the track's top")
eq(w, 6)
local hx, hy, hw, hh = hit_rect(L.bar.thumb_key)
expect(hw > 0 and hh > 0, "its hit region has a width and a height")
eq(count_hits("scroll_track"), 0, "the thumb fills the track: there is no track left to hit")
mark("thumb", x + 3, y + 5)
-- A drag changes nothing: there is nowhere to drag it.
pointer.dispatch({overflow = false, held = true, events = {event("down", L.bar.thumb_key, x + 3, y + 5)}})
pointer.dispatch({overflow = false, held = true, events = {event("move", L.bar.thumb_key, x + 3, y + 400)}})
pointer.dispatch({overflow = false, held = false, events = {event("up", L.bar.thumb_key, x + 3, y + 400)}})
eq(L:scroll_position(), 0, "a drag of a thumb with no travel does nothing")
x, y, w, h = thumb_rect(L)
eq(y, oy) eq(h, 10)
-- The wheel still scrolls, and the thumb stays.
L:scroll({kind = "scroll", unit = "lines", amount = -1})
expect(L:scroll_position() > 0, "the wheel scrolls")
x, y, w, h = thumb_rect(L)
eq(y, oy) eq(h, 10)
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------------------------------
// Use
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn dragging_the_thumb_keeps_the_grabbed_point_under_the_pointer_clamps_at_both_ends_and_releases() {
    let mut rig = rig(
        LIST,
        &[r#"
new_list(20)
local x, y, w, h = thumb_rect(L)
mark("grab", x + 3, y + 4)
"#],
    );
    rig.opened();
    let grab = point(&rig, "grab");
    // Grabbed 4 points below the thumb's top and carried 40 points down: the thumb's top is 40 below the track's top, which is the
    // scroll 390 * 40 / 98 = 159.2, shown as 160.
    press_move_release(&mut rig, grab, (grab.0, grab.1 + 40.0));
    assert_eq!(last(&rig, "s="), "160");
    // The grab was the thumb's: the thumb is now 40 down, and a second drag from its new place by -10 goes 30 down (scroll 119.4).
    let again = (grab.0, grab.1 + 40.0);
    press_move_release(&mut rig, again, (again.0 + 20.0, again.1 - 10.0));
    assert_eq!(last(&rig, "s="), "120");
    // Carried far past the end, then far past the start: clamped, and the scroll is the greatest and then none.
    press_move_release(&mut rig, (grab.0, grab.1 + 30.0), (grab.0, grab.1 + 900.0));
    assert_eq!(last(&rig, "s="), "390");
    let end = (grab.0, grab.1 + 98.0);
    press_move_release(&mut rig, end, (grab.0 + 50.0, grab.1 - 900.0));
    assert_eq!(last(&rig, "s="), "0");
    // Released: a later move scrolls nothing.
    rig.feed(&[PointerEvent::Move(points(grab.0, grab.1 + 60.0))]);
    frame(&mut rig);
    assert_eq!(last(&rig, "s="), "0");
}

#[test]
fn a_press_on_the_track_above_or_below_the_thumb_moves_one_view_and_the_wheel_and_scroll_to_move_the_thumb_with_the_content()
 {
    let mut rig = rig(
        LIST,
        &[
            r#"
new_list(40)
local ox, oy = content_origin()
local x, y, w, h = thumb_rect(L)
-- 40 items: content 1040, share 16.25 -> 16, below the least of 9 pixels: 18; travel 112, greatest scroll 910.
eq(h, 18)
mark("below", x + 3, oy + 120)
L:set_pos(500)
x, y, w, h = thumb_rect(L)
mark("above", x + 3, oy + 2)
L:set_pos(0)
"#,
            r#"
local ox, oy = content_origin()
eq(L:scroll_position(), 130, "a press below the thumb moved one view (130 points) toward the end")
local length, top = expected_thumb(130, 1040, 130)
local x, y, w, h = thumb_rect(L)
eq(y - oy, top, "and the thumb with it")
L:set_pos(500)
"#,
            r#"
eq(L:scroll_position(), 370, "a press above the thumb moved one view toward the start")
L:set_pos(60)
"#,
            r#"
eq(L:scroll_position(), 0, "clamped at the start")
L:scroll({kind = "scroll", unit = "lines", amount = -2})
local ox, oy = content_origin()
eq(L:scroll_position(), 52, "the wheel scrolls as it did")
local length, top = expected_thumb(130, 1040, 52)
local x, y, w, h = thumb_rect(L)
eq(y - oy, top, "and the thumb follows it")
L:scroll_to(40)
length, top = expected_thumb(130, 1040, L:scroll_position())
x, y, w, h = thumb_rect(L)
eq(y - oy, top, "and scroll_to moves the thumb too")
eq(y - oy + h, 130, "to the end")
"#,
        ],
    );
    rig.opened();
    let below = point(&rig, "below");
    rig.feed(&[
        PointerEvent::Down(points(below.0, below.1)),
        PointerEvent::Up(points(below.0, below.1)),
    ]);
    frame(&mut rig);
    rig.opened();
    let above = point(&rig, "above");
    for _ in 0..2 {
        rig.feed(&[
            PointerEvent::Down(points(above.0, above.1)),
            PointerEvent::Up(points(above.0, above.1)),
        ]);
        frame(&mut rig);
        rig.opened();
    }
}

#[test]
fn the_wheel_over_the_bar_or_its_gap_scrolls_the_same_list_and_never_the_map() {
    let mut rig = rig(
        LIST,
        &[r#"
new_list(20)
local ox, oy = content_origin()
mark("track", ox + 237, oy + 120)
mark("gap", ox + 232, oy + 60)
"#],
    );
    rig.opened();
    let camera = crate::map_app::lock(&rig.map).camera();
    let track = point(&rig, "track");
    let gap = point(&rig, "gap");
    // The track is the bar's; the gap beside it is the list's own viewport (no row reaches it).
    assert!(
        rig.key_at(track.0, track.1)
            .is_some_and(|key| key.starts_with("scroll_track_")),
        "{:?}",
        rig.key_at(track.0, track.1)
    );
    assert!(
        rig.key_at(gap.0, gap.1)
            .is_some_and(|key| key.starts_with("list_") && key.ends_with("_area")),
        "{:?}",
        rig.key_at(gap.0, gap.1)
    );
    wheel(&mut rig, track, -1);
    assert_eq!(last(&rig, "s="), "26", "over the track");
    wheel(&mut rig, gap, -1);
    assert_eq!(last(&rig, "s="), "52", "over the gap");
    assert_eq!(crate::map_app::lock(&rig.map).camera(), camera);
}

#[test]
fn the_thumb_is_dim_and_lit_while_hovered_or_dragged() {
    let mut rig = rig(
        LIST,
        &[
            r#"
new_list(20)
local x, y, w, h = thumb_rect(L)
mark("thumb", x + 3, y + 4)
mark("elsewhere", 700, 500)
local _, thumb = parts(L)
eq(thumb.tint, theme_lib.colour("dim"), "dim at rest")
local track = parts(L)
eq(track.tint, theme_lib.colour("track"), "on the track colour")
"#,
            r#"
local _, thumb = parts(L)
eq(thumb.tint, theme_lib.colour("text"), "lit while hovered")
"#,
            r#"
local _, thumb = parts(L)
eq(thumb.tint, theme_lib.colour("dim"), "dim again when the pointer leaves")
"#,
            r#"
local _, thumb = parts(L)
eq(thumb.tint, theme_lib.colour("text"), "lit while dragged, wherever the pointer is")
"#,
            r#"
local _, thumb = parts(L)
eq(thumb.tint, theme_lib.colour("dim"), "dim after the release away from it")
"#,
        ],
    );
    rig.opened();
    let over = point(&rig, "thumb");
    hover(&mut rig, over);
    rig.opened();
    let away = point(&rig, "elsewhere");
    hover(&mut rig, away);
    rig.opened();
    let thumb = point(&rig, "thumb");
    rig.feed(&[
        PointerEvent::Down(points(thumb.0, thumb.1)),
        PointerEvent::Move(points(thumb.0 - 60.0, thumb.1 + 30.0)),
    ]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Up(points(thumb.0 - 60.0, thumb.1 + 30.0))]);
    frame(&mut rig);
    rig.opened();
}

#[test]
fn a_cancel_a_pointer_overflow_the_bar_going_and_the_widget_destroyed_each_end_a_drag_where_it_is()
{
    let mut rig = rig(
        LIST,
        &[r#"
new_list(20)
local ox, oy = content_origin()
local function drag(key, x, y)
  pointer.dispatch({overflow = false, held = true, events = {event("down", key, x, y)}})
  pointer.dispatch({overflow = false, held = true, events = {event("move", key, x, y + 40)}})
end
local x, y = thumb_rect(L)
local key = L.bar.thumb_key
-- a drag moves the content
drag(key, x + 3, y + 4)
eq(L:scroll_position(), 160)
-- a cancel ends it: the next move scrolls nothing
pointer.dispatch({overflow = false, held = false, events = {event("cancel", key, x, y)}})
pointer.dispatch({overflow = false, held = false, events = {event("move", key, x, y + 80)}})
eq(L:scroll_position(), 160, "a cancel ends the drag")
-- an overflow packet ends it: the events of the packet change nothing, and the later move scrolls nothing
x, y = thumb_rect(L)
drag(key, x + 3, y + 4)
local before = L:scroll_position()
pointer.dispatch({overflow = true, held = false, events = {}})
pointer.dispatch({overflow = false, held = false, events = {event("move", key, x, y + 90)}})
eq(L:scroll_position(), before, "an overflow ends the drag, leaving the scroll where it is")
-- the bar going ends it, and it stays ended when the bar returns
x, y = thumb_rect(L)
drag(key, x + 3, y + 4)
local kept = L:scroll_position()
L:set_items(names(3))
eq(count_hits("scroll_"), 0)
L:set_items(names(20))
L:set_pos(kept)
local again = L.bar.thumb_key
pointer.dispatch({overflow = false, held = false, events = {event("move", again, x, y + 100)}})
pointer.dispatch({overflow = false, held = false, events = {event("up", again, x, y + 100)}})
eq(L:scroll_position(), kept, "a drag does not survive the bar going")
-- the widget destroyed during a drag
x, y = thumb_rect(L)
drag(L.bar.thumb_key, x + 3, y + 4)
local dead = L.bar.thumb_key
L:destroy()
eq(pointer.widget(dead), nil, "nothing is registered for the bar's keys")
eq(count_hits("scroll_"), 0)
pointer.dispatch({overflow = false, held = false, events = {event("move", dead, x, y + 100)}})
pointer.dispatch({overflow = false, held = false, events = {event("up", dead, x, y + 100)}})
"#],
    );
    rig.opened();
}

#[test]
fn a_press_on_the_bar_selects_no_row_and_does_not_reach_the_map() {
    let mut rig = rig(
        LIST,
        &[r#"
new_list(20)
local ox, oy = content_origin()
local x, y, w, h = thumb_rect(L)
mark("thumb", x + 3, y + 4)
mark("track", x + 3, oy + 120)
mark("row", ox + 20, oy + 13)
"#],
    );
    rig.opened();
    let camera = crate::map_app::lock(&rig.map).camera();
    let thumb = point(&rig, "thumb");
    press_move_release(&mut rig, thumb, (thumb.0 - 80.0, thumb.1 + 30.0));
    let track = point(&rig, "track");
    rig.feed(&[
        PointerEvent::Down(points(track.0, track.1)),
        PointerEvent::Up(points(track.0, track.1)),
    ]);
    frame(&mut rig);
    assert!(
        !notes(&rig).iter().any(|line| line.starts_with("select ")),
        "{:?}",
        notes(&rig)
    );
    assert_eq!(crate::map_app::lock(&rig.map).camera(), camera);
    // The same press on a row does select it, so the test above is not vacuous.
    let row = point(&rig, "row");
    rig.feed(&[
        PointerEvent::Down(points(row.0, row.1)),
        PointerEvent::Up(points(row.0, row.1)),
    ]);
    frame(&mut rig);
    assert!(
        notes(&rig).iter().any(|line| line.starts_with("select ")),
        "{:?}",
        notes(&rig)
    );
}

#[test]
fn the_fills_and_the_rows_hit_regions_end_5_pixels_short_of_the_view_while_the_bar_shows_and_at_it_otherwise()
 {
    let mut rig = rig(
        LIST,
        &[r#"
new_list(20)
local ox, oy = content_origin()
local function widths()
  local keys, rects = {}, {}
  for index, key in pairs(L.row_keys) do
    local x, y, w, h = hit_rect(key)
    rects[#rects + 1] = w
  end
  return rects
end
for _, w in ipairs(widths()) do eq(w, 230, "a row's hit region") end
-- hover a row, as the pointer does: the fill is a row wide
pointer.dispatch({over = L.row_keys[2], events = {}})
local fills = {}
for _, quad in ipairs(quads_in(L.rows_group)) do
  if quad.h == 26 and quad.x > -1e9 then fills[#fills + 1] = quad end
end
eq(#fills, 1) eq(fills[1].w, 230, "the hover fill ends 5 pixels short")
-- the bar goes: the fills and the regions are as wide as the view again, without a new row being made
L:set_items(names(5))
for _, w in ipairs(widths()) do eq(w, 240, "a row's hit region, no bar") end
fills = {}
for _, quad in ipairs(quads_in(L.rows_group)) do
  if quad.h == 26 and quad.x > -1e9 then fills[#fills + 1] = quad end
end
eq(#fills, 1) eq(fills[1].w, 240, "and the fill")
L:set_items(names(20))
for _, w in ipairs(widths()) do eq(w, 230) end
"#],
    );
    rig.opened();
}

// ---------------------------------------------------------------------------------------------------------------------
// The card stack
// ---------------------------------------------------------------------------------------------------------------------

const STACK: &str = r#"
cards = require("lib.cards")
drawn_widths = {}
function stack_items(count)
  local items = {}
  for i = 1, count do
    items[i] = {height = 20, draw = function(group, x, y, width)
      drawn_widths[#drawn_widths + 1] = width
      return {destroy = function() end}
    end}
  end
  return items
end
function new_stack(count)
  new_window(120, 100)
  drawn_widths = {}
  L = cards.new{window = W, x = 0, y = 0, width = 120 * P, height = 60 * P, gap = 1, items = stack_items(count)}
  return L
end
"#;

#[test]
fn a_card_stack_has_the_bar_while_its_items_and_gaps_are_taller_than_its_view_and_draws_each_item_at_the_row_width()
 {
    let mut rig = rig(
        &format!("{LIST}{STACK}"),
        &[r#"
-- The view is 120 points: 3 cards of 20 pixels and their 2 gaps of 1 pixel are 124 points.
new_stack(2)
local ox, oy = content_origin()
eq(#quads_in(L.bar.group), 0, "two cards fit")
for _, w in ipairs(drawn_widths) do eq(w, 240, "drawn the width of the view") end
L:set_items(stack_items(3), 1)
eq(#quads_in(L.bar.group), 2, "three cards and their gaps are 124 > 120")
eq(count_hits("scroll_"), 2)
for _, w in ipairs(drawn_widths) do expect(w == 240 or w == 230, "a width") end
eq(drawn_widths[#drawn_widths], 230, "every item is drawn again at the narrower width")
local x, y, w, h = track_rect(L)
eq(x, ox + 234) eq(y, oy) eq(w, 6) eq(h, 120)
local length, top = expected_thumb(120, 124, 0)
local tx, ty, tw, th = thumb_rect(L)
eq(th, length, "the thumb is 9 pixels at least: 18, never over the view") eq(ty - oy, top)
L:set_items(stack_items(2), 1)
eq(#quads_in(L.bar.group), 0, "the bar goes when the content fits again")
eq(drawn_widths[#drawn_widths], 240, "and every item is drawn again at the full width")
L:set_items(stack_items(10), 1)
eq(#quads_in(L.bar.group), 2)
-- The items' hit regions are as wide as the items are drawn.
for key in pairs(L.item_keys) do
  local x, y, w, h = hit_rect(key)
  eq(w, 230, "an item's hit region")
end
"#],
    );
    rig.opened();
}

#[test]
fn the_wheel_the_track_and_a_drag_scroll_a_card_stack_and_its_thumb_follows() {
    let mut rig = rig(
        &format!("{LIST}{STACK}"),
        &[
            r#"
new_stack(10)
local ox, oy = content_origin()
-- 10 cards of 40 points and 9 gaps of 2 are 418 points in a view of 120: greatest scroll 298.
local length, top = expected_thumb(120, 418, 0)
local x, y, w, h = thumb_rect(L)
eq(h, length)
mark("thumb", x + 3, y + 3)
mark("track", x + 3, oy + 110)
L:scroll({kind = "scroll", unit = "lines", amount = -1})
eq(L:scroll_position(), 26)
length, top = expected_thumb(120, 418, 26)
x, y, w, h = thumb_rect(L)
eq(y - oy, top, "the wheel moves the thumb")
L:set_pos(0)
"#,
            r"
-- A press on the track below the thumb moved one view (120 points).
eq(L:scroll_position(), 120)
L:set_pos(0)
",
        ],
    );
    rig.opened();
    let track = point(&rig, "track");
    rig.feed(&[
        PointerEvent::Down(points(track.0, track.1)),
        PointerEvent::Up(points(track.0, track.1)),
    ]);
    frame(&mut rig);
    rig.opened();
    // A drag of the thumb: 28 points down is a scroll of 298 * 28 / travel (the thumb is 34 long, so its travel is 120 - 34 = 86).
    let thumb = point(&rig, "thumb");
    rig.feed(&[
        PointerEvent::Down(points(thumb.0, thumb.1)),
        PointerEvent::Move(points(thumb.0, thumb.1 + 28.0)),
        PointerEvent::Up(points(thumb.0, thumb.1 + 28.0)),
    ]);
    frame(&mut rig);
    // 298 * 28 / 86 = 97.02: a card stack keeps the scroll it is given and shows it to the pixel.
    let scroll: f64 = last(&rig, "s=").parse().unwrap();
    assert!((scroll - 97.02).abs() < 0.01, "{scroll}");
}
