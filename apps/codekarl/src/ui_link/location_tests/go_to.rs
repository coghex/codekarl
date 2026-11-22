//! Going to things on the map (CK-381) in a real UI VM: the ledger's right-click, the location window's title-bar go-to button and the
//! function that goes and opens, over the stub `game` of the location tests whose `game.go_to` records its calls.
//!
//! A script step runs in the VM when the test posts `Opened`; the tests read what the steps `note`d. Nothing sleeps.

use super::characters::{CHARACTERS, card_point};
use super::*;
use ck_window::PointerButton;

/// The Lua after the characters' prelude: the calls of `game.go_to` as one note, the words of every label tooltip by key, and the
/// geometry of a location window's title bar.
const GO_PRELUDE: &str = r##"
-- Later revisions show what revision 2 does: the tests that refresh the ledger many times need views for them.
setmetatable(VIEWS, {__index = function() return rawget(VIEWS, 2) end})
function announce_go()
  local parts = {}
  for index, call in ipairs(go_to_calls) do
    local id = call.id
    if type(id) == "table" then id = id.place .. "#" .. tostring(id.slot) end
    parts[index] = string.format("%s:%s:%s:%s", call.kind, tostring(id), tostring(call.zoom), tostring(call.revision))
  end
  note("go " .. table.concat(parts, ";"))
end
label_words = {}
do
  local tooltip_library = require("lib.tooltip")
  local real_label = tooltip_library.label
  tooltip_library.label = function(key, words)
    label_words[key] = words
    return real_label(key, words)
  end
end
-- Whether the window opened for `name` (a place's name) is open.
function announce_open(name)
  local window = window_objects[name]
  note("open " .. name .. " " .. tostring(window ~= nil and not window.closed))
end
-- The title bar of the location window of `name`: the go-to button's rectangle and tooltip, and where the tabs and the close button lie.
function announce_title(name)
  local window = window_objects[name]
  local x, y = window:position()
  local go = go_buttons[#go_buttons]
  local go_width, go_height = go:size()
  local tabs_object = tabs_by_title[name]
  local last = tabs_object.buttons[#tabs_object.buttons]
  local first = tabs_object.buttons[1]
  local close = window.close_button
  note(string.format("title %g %g %g %g tip=%s tabs=%g..%g go=%g..%g close=%g..%g width=%g tab_text=%s", x + go.x, y + go.y, go_width, go_height,
    tostring(label_words[go.key]), first.x, last.x + last:size(), go.x, go.x + go_width, close.x, close.x + close.width, window.outer_width,
    tostring(tabs_object.items[1].labels[tabs_object.tier])))
end
"##;

fn go_rig(steps: &[&str], width: u32, height: u32) -> Rig {
    rig_with_prelude_at(
        &format!("{CHARACTERS}\n{GO_PRELUDE}"),
        steps,
        width,
        height,
        2,
    )
}

fn right_click(rig: &mut Rig, point: (f64, f64)) {
    right_click_unseen(rig, point);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

/// A right-click routed by the input link but not yet delivered to the UI (no `frame_update` has carried it).
fn right_click_unseen(rig: &mut Rig, (x, y): (f64, f64)) {
    for button_event in [
        PointerEvent::ButtonDown {
            button: PointerButton::Right,
            position: points(x, y),
        },
        PointerEvent::ButtonUp {
            button: PointerButton::Right,
            position: points(x, y),
        },
    ] {
        rig.feed(&[button_event]);
    }
}

/// The last `go ...` note: the calls of `game.go_to` so far.
fn go_calls(rig: &Rig) -> String {
    notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("go ").map(str::to_owned))
        .expect("a go note")
}

fn open_state(rig: &Rig, name: &str) -> bool {
    let wanted = format!("open {name} ");
    notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix(&wanted).map(|state| state == "true"))
        .unwrap_or_else(|| panic!("no open note for {name}"))
}

/// The point at the left of the `n`th (1-based, the header counted) row of the ledger's list.
fn ledger_row_point(rig: &Rig, row: usize) -> (f64, f64) {
    let (x, y) = ledger_origin(rig);
    let row = f64::from(u32::try_from(row).expect("fixture rows fit in u32"));
    (x + 20.0, y + (row - 0.5) * 13.0 * 2.0)
}

/// The 1-based number, among the last rows noted, of the row that starts with `prefix`.
fn row_number(rig: &Rig, prefix: &str) -> usize {
    let last = rows(rig).last().expect("rows").clone();
    last.split('|')
        .position(|row| row.starts_with(prefix))
        .unwrap_or_else(|| panic!("no row starting {prefix}: {last}"))
        + 1
}

#[test]
fn a_right_click_on_a_places_row_goes_to_it_without_zoom_and_opens_nothing_where_a_left_click_still_opens()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "announce_go() announce_open('Rowan')",
            "announce_go() announce_open('Rowan')",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let row = row_number(&rig, "Rowan@1:left");
    let point = ledger_row_point(&rig, row);
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(go_calls(&rig), "location:rowan:false:1");
    assert!(
        !open_state(&rig, "Rowan"),
        "a right-click opens nothing and selects nothing"
    );
    // The left click opens the place as before and asks for no go-to.
    click_ledger_row(&mut rig, row, 1800, 900);
    rig.opened();
    assert_eq!(go_calls(&rig), "location:rowan:false:1");
    assert!(open_state(&rig, "Rowan"), "a left click opens the window");
}

#[test]
fn a_right_click_on_a_pops_row_goes_to_its_place_and_slot_and_on_a_header_or_empty_space_does_nothing()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "announce_go()",
            "announce_go()",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    // Cluain Mhic Nóis (`fada`) holds two pops: the second, a monk's, is slot 1.
    let monk = row_number(&rig, "monk mide@");
    let point = ledger_row_point(&rig, monk);
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(go_calls(&rig), "pop:fada#1:false:1");
    let mystery = row_number(&rig, "mystery ");
    let point = ledger_row_point(&rig, mystery);
    right_click(&mut rig, point);
    rig.opened();
    assert!(
        go_calls(&rig).ends_with(";pop:fada#0:false:1"),
        "{}",
        go_calls(&rig)
    );
    let calls = go_calls(&rig);
    // Well below the rows: the list's empty space.
    let (x, y) = ledger_origin(&rig);
    right_click(&mut rig, (x + 20.0, y + 700.0));
    rig.opened();
    assert_eq!(go_calls(&rig), calls, "empty space does nothing");
}

#[test]
fn a_right_click_on_the_places_header_or_the_scroll_bar_does_nothing() {
    let mut rig = go_rig(
        &[
            "use_many_ledger_places() ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "announce_go()",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    // The header is the first row; it stands for no place.
    let point = ledger_row_point(&rig, 1);
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(go_calls(&rig), "");
    // The 48 places overflow the list, so its scroll bar lies at the right edge of the content.
    let (x, y) = ledger_origin(&rig);
    let content_width = ledger::ledger_content_width(&rig);
    let bar = (x + content_width - 3.0, y + 3.0 * 13.0 * 2.0);
    assert!(
        rig.key_at(bar.0, bar.1)
            .is_none_or(|key| !key.contains("_row_")),
        "the point is on the scroll bar, not on a row"
    );
    right_click(&mut rig, bar);
    rig.opened();
    assert_eq!(go_calls(&rig), "", "the scroll bar is no row");
}

#[test]
fn a_right_click_on_a_character_card_goes_to_that_character_without_zoom_and_opens_no_screen() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_cards() announce_ledger() announce_ledger_tabs()",
            "announce_go() announce_screen()",
            "announce_go() announce_screen()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "characters");
    rig.opened();
    // Cards come in importance order: the first is Niall (1), the third the abbot (3).
    let point = card_point(&rig, 3);
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(go_calls(&rig), "character:3:false:1");
    assert!(
        notes(&rig).iter().any(|line| line == "screen closed"),
        "a right-click opens no character screen"
    );
}

#[test]
fn a_row_of_a_replaced_world_does_nothing_and_a_refresh_draws_rows_for_the_new_revision() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            // A load: the same revision, but world 1. The rows were drawn in world 0 and are not redrawn for it.
            "ctime(2, 1, 1)",
            "announce_go()",
            "ctime(3, 2, 1)",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let row = row_number(&rig, "Rowan@1:left");
    rig.opened();
    let point = ledger_row_point(&rig, row);
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(go_calls(&rig), "", "a row of a replaced world does nothing");
    // A new revision draws the rows again, in the world and the revision the list now holds.
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    let row = row_number(&rig, "Rowan@1:left");
    let point = ledger_row_point(&rig, row);
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(go_calls(&rig), "location:rowan:false:2");
}

#[test]
fn the_title_bar_has_a_go_to_button_with_a_tooltip_whose_click_goes_with_zoom_and_leaves_the_window_open()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) select_place('rowan', 'Rowan')",
            "announce_title('Rowan')",
            "announce_go() announce_open('Rowan')",
        ],
        1800,
        900,
    );
    rig.opened();
    rig.opened();
    let title = notes(&rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("title ").map(str::to_owned))
        .expect("the title bar note");
    assert!(title.contains("tip=go to"), "{title}");
    let numbers: Vec<f64> = title
        .split_whitespace()
        .take(4)
        .map(|part| part.parse().unwrap())
        .collect();
    left_click(
        &mut rig,
        (numbers[0] + numbers[2] / 2.0, numbers[1] + numbers[3] / 2.0),
    );
    rig.opened();
    assert_eq!(go_calls(&rig), "location:rowan:true:1");
    assert!(open_state(&rig, "Rowan"), "the window stays open");
}

/// A left click at the point (the title button takes a left click).
fn left_click(rig: &mut Rig, (x, y): (f64, f64)) {
    rig.feed(&[
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
}

fn title_numbers(rig: &Rig) -> (f64, f64, f64, f64, f64, f64, f64) {
    let title = notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("title ").map(str::to_owned))
        .expect("the title bar note");
    let field = |name: &str| -> (f64, f64) {
        let text = title
            .split_whitespace()
            .find_map(|part| part.strip_prefix(name))
            .unwrap_or_else(|| panic!("no {name} in {title}"));
        let (low, high) = text.split_once("..").expect("a range");
        (low.parse().unwrap(), high.parse().unwrap())
    };
    let tabs = field("tabs=");
    let go = field("go=");
    let close = field("close=");
    let width: f64 = title
        .split_whitespace()
        .find_map(|part| part.strip_prefix("width="))
        .unwrap()
        .parse()
        .unwrap();
    (tabs.0, tabs.1, go.0, go.1, close.0, close.1, width)
}

#[test]
fn on_the_narrowest_panel_with_a_long_name_the_tabs_the_go_to_button_and_the_close_button_stay_apart_and_usable()
 {
    let long = "\u{D3}".repeat(40);
    // A window 300 points wide: a third is narrower than the panel's floor, so the panel is at its narrowest.
    let mut rig = go_rig(
        &[
            &format!("ctime(1, 1, 0) select_place('long', '{long}')"),
            &format!("announce_title('{long}')"),
            &format!("announce_go() announce_open('{long}')"),
        ],
        300,
        600,
    );
    rig.opened();
    rig.opened();
    let (tabs_start, tabs_end, go_start, go_end, close_start, close_end, width) =
        title_numbers(&rig);
    assert!(
        tabs_start >= 0.0 && tabs_end <= go_start,
        "tabs {tabs_start}..{tabs_end} against the button at {go_start}"
    );
    assert!(
        go_end <= close_start,
        "the button ends at {go_end}, the close button starts at {close_start}"
    );
    assert!(
        close_end <= width,
        "the close button ends at {close_end} in a window {width} wide"
    );
    // Every control has width, and the name has clipped to make the room (the first tab shows its tiny label).
    assert!(tabs_end > tabs_start && go_end > go_start && close_end > close_start);
    let title = notes(&rig)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("title "))
        .unwrap();
    assert!(
        title.ends_with("tab_text=\u{D3}"),
        "the name clips first: {title}"
    );
    // The button still works there.
    let numbers: Vec<f64> = title
        .split_whitespace()
        .skip(1)
        .take(4)
        .map(|part| part.parse().unwrap())
        .collect();
    left_click(
        &mut rig,
        (numbers[0] + numbers[2] / 2.0, numbers[1] + numbers[3] / 2.0),
    );
    rig.deliver_fixture_frame_at(300, 600, 2);
    rig.opened();
    assert_eq!(go_calls(&rig), "location:long:true:1");
}

#[test]
fn the_open_function_goes_with_zoom_then_opens_the_location_the_pops_tab_or_the_character_screen_and_for_a_river_only_goes()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "require('lib.go_to').to('location', 'rowan', 1) announce_go() announce_open('Rowan')",
            // The location panel is one window: opening the pop shows its place in it, on the pops tab.
            "require('lib.go_to').to('pop', {place = 'fada', slot = 1}, 1) announce_go()
             note('shows ' .. tabs_by_title['Rowan'].items[1].labels[1] .. ' on ' .. tabs_by_title['Rowan']:current())",
            "require('lib.go_to').to('character', 1, 1) announce_go() announce_screen()",
            "local before = 0 for _ in pairs(window_objects) do before = before + 1 end
             windows_before = before
             require('lib.go_to').to('river', 'erne', 1) announce_go()
             local after = 0 for _ in pairs(window_objects) do after = after + 1 end
             note('windows ' .. before .. ' ' .. after)",
        ],
        1800,
        900,
    );
    rig.opened();
    rig.opened();
    assert_eq!(go_calls(&rig), "location:rowan:true:1");
    assert!(open_state(&rig, "Rowan"));
    rig.opened();
    assert!(
        go_calls(&rig).ends_with(";pop:fada#1:true:1"),
        "{}",
        go_calls(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "shows Cluain Mhic N\u{f3}is on pops"),
        "the pops tab of that place shows: {:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.starts_with("shows"))
            .collect::<Vec<_>>()
    );
    rig.opened();
    assert!(
        go_calls(&rig).ends_with(";character:1:true:1"),
        "{}",
        go_calls(&rig)
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("screen id=1 ")),
        "the character screen is open for the character: {:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.starts_with("screen"))
            .collect::<Vec<_>>()
    );
    rig.opened();
    assert!(
        go_calls(&rig).ends_with(";river:erne:true:1"),
        "{}",
        go_calls(&rig)
    );
    assert!(
        notes(&rig).iter().any(|line| {
            line.strip_prefix("windows ").is_some_and(|counts| {
                counts
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .all(|pair| pair[0] == pair[1])
            })
        }),
        "a river opens no window"
    );
}

#[test]
fn a_refused_go_to_opens_nothing_and_the_opener_reads_the_revision_it_was_given() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            // The UI now holds revision 2; the thing is named in revision 1 (a held, older one).
            "ctime(2, 2, 0)
             go_to_refusal = 'revision 1 is retired'
             local ok, reason = require('lib.go_to').to('location', 'fada', 1)
             note('result ' .. tostring(ok) .. ' ' .. tostring(reason))
             announce_open('Cluain Mhic N\u{f3}is')",
            "go_to_refusal = nil
             require('lib.go_to').to('character', 1, 1)
             note('calls ' .. table.concat(character_calls, ','))",
        ],
        1800,
        900,
    );
    rig.opened();
    rig.opened();
    assert!(
        notes(&rig).iter().any(
            |line| line == "result false game.go_to: revision 1 is retired"
                || line.starts_with("result false") && line.ends_with("revision 1 is retired")
        ),
        "{:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.starts_with("result"))
            .collect::<Vec<_>>()
    );
    assert!(
        !open_state(&rig, "Cluain Mhic N\u{f3}is"),
        "a refusal opens no window"
    );
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "calls 1@1"),
        "the character is read in the revision it was named in, not the newest: {:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.starts_with("calls"))
            .collect::<Vec<_>>()
    );
}

/// The text of the last note that starts with `prefix`, without it.
fn note_after(rig: &Rig, prefix: &str) -> String {
    notes(rig)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix(prefix).map(str::to_owned))
        .unwrap_or_else(|| panic!("no {prefix} note"))
}

#[test]
fn the_location_and_pop_openers_read_the_revision_the_camera_went_under_not_the_panels() {
    let mut rig = go_rig(
        &[
            // The UI holds revision 2; the go-to is made under revision 1 (the stub accepts it).
            "ctime(1, 1, 0) ctime(2, 2, 0) announce_bars()",
            "reads_before = #place_calls
             require('lib.go_to').to('location', 'rowan', 1)
             note('reads_location ' .. table.concat(place_calls, ',', reads_before + 1))",
            "reads_before = #place_calls
             require('lib.go_to').to('pop', {place = 'fada', slot = 1}, 1)
             note('reads_pop ' .. table.concat(place_calls, ',', reads_before + 1))",
            // The next revision `game_time` carries refreshes the panel as it always does.
            "reads_before = #place_calls
             ctime(3, 2, 0)
             note('reads_after ' .. table.concat(place_calls, ',', reads_before + 1))",
        ],
        1800,
        900,
    );
    rig.opened();
    rig.opened();
    let location = note_after(&rig, "reads_location ");
    assert!(
        location.contains("rowan@1") && !location.contains("@2"),
        "the location window reads revision 1: {location}"
    );
    rig.opened();
    let pop = note_after(&rig, "reads_pop ");
    assert!(
        pop.contains("fada@1") && !pop.contains("@2"),
        "the pops tab reads revision 1: {pop}"
    );
    rig.opened();
    let after = note_after(&rig, "reads_after ");
    assert!(
        after.contains("@2") && !after.contains("@1"),
        "a later delivery refreshes the panel for the newer revision: {after}"
    );
}

/// The two ways a right-click routed against one frame can reach a ledger that has since been refreshed.
fn queued_right_click(world_after: u32) -> Vec<String> {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            // The refresh: another revision (and maybe another world) with the places in another order, so that row 3 holds another pop.
            &format!(
                "place_data = {{place_data[2], place_data[3], place_data[4], place_data[1]}} ctime(2, 2, {world_after})"
            ),
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    // Row 3 of the pops tab is the monk of Cluain Mhic Nóis (`fada`, slot 1) in revision 1.
    let monk = row_number(&rig, "monk mide@");
    assert_eq!(monk, 3);
    let point = ledger_row_point(&rig, monk);
    right_click_unseen(&mut rig, point);
    // The refresh happens before the UI is given the packet that carries the click.
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    vec![go_calls(&rig)]
}

#[test]
fn a_right_click_routed_before_a_refresh_goes_to_the_row_it_was_routed_for_in_the_revision_it_was_drawn()
 {
    assert_eq!(queued_right_click(0), ["pop:fada#1:false:1"]);
}

#[test]
fn a_right_click_routed_before_a_load_and_delivered_after_it_does_nothing() {
    assert_eq!(queued_right_click(1), [""]);
}

#[test]
fn a_left_click_whose_press_and_release_straddle_a_refresh_of_the_same_row_still_opens_the_place() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            // A same-world refresh: another revision, the same places in the same order.
            "ctime(2, 2, 0)",
            "announce_open('Rowan')",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let row = row_number(&rig, "Rowan@1:left");
    let (x, y) = ledger_row_point(&rig, row);
    rig.feed(&[PointerEvent::Down(points(x, y))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.feed(&[PointerEvent::Up(points(x, y))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(open_state(&rig, "Rowan"), "the click opened the place");
}

/// A pop right-clicked at revision 1 whose row is unchanged by the refresh (the same pop at the same place in the list): the click routed
/// before the refresh keeps the revision it was drawn in, and a click made after it reads the new one.
#[test]
fn a_queued_right_click_on_an_unchanged_row_keeps_its_draw_revision_and_a_fresh_one_reads_the_new_one()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "ctime(2, 2, 0)",
            "announce_go()",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let monk = row_number(&rig, "monk mide@");
    let point = ledger_row_point(&rig, monk);
    right_click_unseen(&mut rig, point);
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert_eq!(go_calls(&rig), "pop:fada#1:false:1", "the queued click");
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(
        go_calls(&rig),
        "pop:fada#1:false:1;pop:fada#1:false:2",
        "a click made after the refresh"
    );
}

fn queued_left_press(tick_before_delivery: bool) {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "ctime(2, 2, 0)",
            "announce_open('Rowan')",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let row = row_number(&rig, "Rowan@1:left");
    let (x, y) = ledger_row_point(&rig, row);
    // The input link routes the press against the rows of revision 1; the refresh happens before the UI is given it.
    rig.feed(&[PointerEvent::Down(points(x, y))]);
    rig.opened();
    // An input tick comes before the UI is given the press: the link still owns the press's key, which must still be in the scene.
    if tick_before_delivery {
        rig.feed(&[]);
    }
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.feed(&[PointerEvent::Up(points(x, y))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(open_state(&rig, "Rowan"), "the click opened the place");
}

#[test]
fn a_left_press_routed_before_a_refresh_and_given_to_the_ui_after_it_still_ends_in_a_click() {
    queued_left_press(false);
}

#[test]
fn the_same_press_still_ends_in_a_click_when_an_input_tick_comes_between_the_refresh_and_its_delivery()
 {
    queued_left_press(true);
}

#[test]
fn a_fresh_right_click_over_a_row_that_a_refresh_removed_does_nothing_even_while_a_left_press_is_held()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            // The refresh leaves the Pops ledger empty.
            "place_data = {} ctime(2, 2, 0)",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let monk = row_number(&rig, "monk mide@");
    let point = ledger_row_point(&rig, monk);
    // The left press on the monk has been given to the UI when the list is refreshed away under it.
    rig.feed(&[PointerEvent::Down(points(point.0, point.1))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    // Still holding the left button, a fresh right-click lands where the row was.
    right_click(&mut rig, point);
    rig.opened();
    assert_eq!(
        go_calls(&rig),
        "",
        "the row has gone: nothing is there to go to"
    );
}

/// How many regions of the list whose row `key` is are published (the click that opens the place adds the place window's own lists, which
/// are not counted).
fn row_hit_count(rig: &Rig, key: &str) -> usize {
    let prefix = format!("{}_row_", key.split("_row_").next().expect("a row key"));
    rig.harness
        .bridge
        .hit_regions()
        .expect("published regions")
        .regions()
        .iter()
        .filter(|region| region.key().starts_with(&prefix))
        .count()
}

#[test]
fn a_press_and_its_release_both_routed_before_a_refresh_is_delivered_still_end_in_a_click() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "ctime(2, 2, 0)",
            "announce_open('Rowan')",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let row = row_number(&rig, "Rowan@1:left");
    let (x, y) = ledger_row_point(&rig, row);
    rig.feed(&[PointerEvent::Down(points(x, y))]);
    // The refresh, then the release routed against the refreshed rows, and only then is the UI given both events.
    rig.opened();
    rig.feed(&[PointerEvent::Up(points(x, y))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(open_state(&rig, "Rowan"), "the click opened the place");
}

#[test]
fn a_fresh_right_click_on_a_row_an_empty_refresh_removed_does_nothing_at_any_point_of_it() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "place_data = {} ctime(2, 2, 0)",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let monk = row_number(&rig, "monk mide@");
    let (x, y) = ledger_row_point(&rig, monk);
    rig.opened();
    // Before the UI has been given anything: the corners and the middle of the row that was.
    for point in [(x, y), (x + 150.0, y - 12.0), (x + 300.0, y + 12.0)] {
        right_click_unseen(&mut rig, point);
    }
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert_eq!(go_calls(&rig), "");
}

#[test]
fn a_right_click_routed_while_a_left_press_is_held_and_given_to_the_ui_after_a_refresh_still_goes_to_its_row()
 {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "ctime(2, 2, 0)",
            "announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let monk = row_number(&rig, "monk mide@");
    let point = ledger_row_point(&rig, monk);
    rig.feed(&[PointerEvent::Down(points(point.0, point.1))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    // The right-click is routed while the left button is held and the pop is still drawn; the refresh comes before the UI is given it.
    right_click_unseen(&mut rig, point);
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.feed(&[PointerEvent::Up(points(point.0, point.1))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert_eq!(go_calls(&rig), "pop:fada#1:false:1");
}

#[test]
fn two_refreshes_during_one_hold_leave_no_hit_region_behind_after_the_release() {
    let mut rig = go_rig(
        &[
            "ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            "ctime(2, 2, 0)",
            "ctime(3, 3, 0)",
            "announce_open('Rowan')",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    let row = row_number(&rig, "Rowan@1:left");
    let (x, y) = ledger_row_point(&rig, row);
    let key = rig.key_at(x, y).expect("a row is hit");
    let before = row_hit_count(&rig, &key);
    rig.feed(&[PointerEvent::Down(points(x, y))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.feed(&[PointerEvent::Up(points(x, y))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert_eq!(
        row_hit_count(&rig, &key),
        before,
        "the same regions as before the hold"
    );
    assert!(open_state(&rig, "Rowan"), "the click opened the place");
}

/// A right-click on the monk of `fada` routed against revision 1, then `count` refreshes before the UI is given it: what it goes to.
fn queued_across_refreshes(count: u32) -> String {
    let mut refresh_steps = String::new();
    for revision in 2..2 + count {
        write!(refresh_steps, "ctime({revision}, {revision}, 0) ").expect("a string takes text");
    }
    let steps = [
        "ctime(1, 1, 0) announce_bars()".to_owned(),
        "announce_ledger()".to_owned(),
        refresh_steps,
        "announce_go()".to_owned(),
    ];
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    let mut rig = go_rig(&steps, 1800, 900);
    rig.opened();
    press_bar(&mut rig, "pops");
    rig.opened();
    let monk = row_number(&rig, "monk mide@");
    let point = ledger_row_point(&rig, monk);
    right_click_unseen(&mut rig, point);
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    go_calls(&rig)
}

#[test]
fn a_queued_right_click_keeps_its_target_across_any_number_of_refreshes() {
    // The event carries what its row stood for: nothing expires with the number of refreshes between routing and dispatch.
    assert_eq!(queued_across_refreshes(2), "pop:fada#1:false:1");
    assert_eq!(queued_across_refreshes(9), "pop:fada#1:false:1");
    assert_eq!(queued_across_refreshes(40), "pop:fada#1:false:1");
}

#[test]
fn a_tagged_right_press_whose_row_and_widget_have_gone_still_goes_to_the_thing_its_tag_names() {
    let mut rig = go_rig(
        &[
            "use_many_ledger_places() ctime(1, 1, 0) announce_bars()",
            "announce_ledger()",
            // The ledger list's third row key, before the list is scrolled.
            "stale_key = list_object.row_keys[3] note('stale_key ' .. stale_key)",
            // After the scroll has been dispatched and its retired rows flushed, the widget of that key is gone.
            "note('widget_gone ' .. tostring(require('lib.pointer').widget(stale_key) == nil) .. ' scrolled ' .. list_object:scroll_position())",
            // A right press routed against the earlier drawing reaches the UI now: its tag is the whole target.
            "require('lib.pointer').dispatch({events = {{kind = 'button_down', button = 'right', key = stale_key, inside = false, x = 0, y = 0,
               tag = 'go:location:rowan:1:0'}}, overflow = false, over = false})
             require('lib.pointer').dispatch({events = {{kind = 'button_down', button = 'right', key = stale_key, inside = false, x = 0, y = 0,
               tag = 'go:location:rowan:1:1'}}, overflow = false, over = false})
             require('lib.pointer').dispatch({events = {{kind = 'button_down', button = 'right', key = stale_key, inside = false, x = 0, y = 0,
               tag = 'go:location:rowan:1:0'}}, overflow = true, over = false})
             require('lib.pointer').dispatch({events = {{kind = 'button_down', button = 'middle', key = stale_key, inside = false, x = 0, y = 0,
               tag = 'go:location:rowan:1:0'}}, overflow = false, over = false})
             require('lib.pointer').dispatch({events = {{kind = 'button_down', button = 'right', key = stale_key, inside = false, x = 0, y = 0,
               tag = 'not a go-to tag'}}, overflow = false, over = false})
             announce_go()",
        ],
        1800,
        900,
    );
    rig.opened();
    press_bar(&mut rig, "location");
    rig.opened();
    rig.opened();
    let (x, y) = ledger_origin(&rig);
    // Scroll the 48-row ledger well down, so that its third row leaves the view and its widget is retired.
    rig.feed(&[
        scroll_lines_at(x + 20.0, y + 45.0, -12),
        scroll_lines_at(x + 20.0, y + 45.0, -12),
    ]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    // A later pointer packet ends the dispatch that retired the scrolled-out rows' widgets.
    rig.feed(&[PointerEvent::Move(points(x + 30.0, y + 60.0))]);
    rig.deliver_fixture_frame_at(1800, 900, 2);
    rig.opened();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line.starts_with("widget_gone true")),
        "the row's widget has gone: {:?}",
        notes(&rig)
            .iter()
            .filter(|line| line.starts_with("widget_gone") || line.starts_with("stale_key"))
            .collect::<Vec<_>>()
    );
    rig.opened();
    assert_eq!(
        go_calls(&rig),
        "location:rowan:false:1",
        "only the right press of this world, in an eligible packet, with a go-to tag, does anything"
    );
}

fn scroll_lines_at(x: f64, y: f64, amount: i32) -> PointerEvent {
    PointerEvent::Scroll {
        position: points(x, y),
        amount: amount * 256,
        unit: ck_window::ScrollUnit::Lines,
    }
}

#[test]
fn a_tag_is_built_only_from_a_thing_that_can_be_named_in_it() {
    // The tag alphabet is 1 to 128 printable ASCII characters (`render.set_hit_tag`); the builder never offers anything else.
    let mut rig = go_rig(
        &[
            "local t = require('lib.go_to')
             local long = string.rep('a', 120)
             note('tags ' .. table.concat({
               tostring(t.tag('location', 'clonmacnoise', 1795, 12)),
               tostring(t.tag('pop', {place = 'clonmacnoise', slot = 1}, 1795, 12)),
               tostring(t.tag('character', 4096, 9007199254740, 3)),
               tostring(t.tag('river', 'erne', 7, 0)),
               tostring(t.tag('location', 'has space', 1, 0)),
               tostring(t.tag('location', 'colon:id', 1, 0)),
               tostring(t.tag('location', 'caf\u{e9}', 1, 0)),
               tostring(t.tag('location', long, 1, 0)),
               tostring(t.tag('pop', {place = 'a', slot = -1}, 1, 0)),
               tostring(t.tag('location', 'rowan', 0, 0)),
               tostring(t.tag('merchant', 'x', 1, 0)),
             }, '|'))
             local kind, id, revision, world = t.parse('go:pop:clonmacnoise#1:1795:12')
             note('parsed ' .. kind .. ' ' .. id.place .. ' ' .. id.slot .. ' ' .. revision .. ' ' .. world)
             note('unparsed ' .. tostring(t.parse('go:location:rowan:1')) .. ' ' .. tostring(t.parse('go:pop:nohash:1:0')))",
        ],
        1800,
        900,
    );
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line
            == "tags go:location:clonmacnoise:1795:12|go:pop:clonmacnoise#1:1795:12|go:character:4096:9007199254740:3|go:river:erne:7:0|nil|nil|nil|nil|nil|nil|nil"),
        "{:?}",
        notes(&rig).iter().filter(|line| line.starts_with("tags")).collect::<Vec<_>>()
    );
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "parsed pop clonmacnoise 1 1795 12")
    );
    assert!(notes(&rig).iter().any(|line| line == "unparsed nil nil"));
}

/// The tag the published regions give the point `(x, y)`.
fn tag_at(rig: &Rig, x: f64, y: f64) -> Option<String> {
    rig.harness
        .bridge
        .hit_regions()
        .expect("published regions")
        .tag_at(points(x, y))
        .map(str::to_owned)
}

#[test]
fn a_tagged_right_press_is_stopped_by_the_pointer_gate_like_any_keyed_event() {
    let mut rig = go_rig(
        &[
            // A closed gate (the Save/Load screen sets one while a load is pending) swallows keyed events before any widget hears them.
            "local pointer = require('lib.pointer')
             local event = function(tag) return {events = {{kind = 'button_down', button = 'right', key = 'some_key', inside = false,
               x = 0, y = 0, tag = tag}}, overflow = false, over = false} end
             pointer.set_gate(function() return true end)
             pointer.dispatch(event('go:location:rowan:1:0'))
             note('gated ' .. #go_to_calls)
             pointer.set_gate(nil)
             pointer.dispatch(event('go:location:rowan:1:0'))
             note('open ' .. #go_to_calls)",
        ],
        1800,
        900,
    );
    rig.opened();
    assert!(
        notes(&rig).iter().any(|line| line == "gated 0"),
        "the gate stops it"
    );
    assert!(
        notes(&rig).iter().any(|line| line == "open 1"),
        "without the gate it goes"
    );
}

#[test]
fn a_tag_the_scene_refuses_takes_the_rows_earlier_tag_with_it() {
    let mut rig = go_rig(
        &[
            "local windows = require('lib.window')
             local lists = require('lib.list')
             tag_window = windows.new{title = 'tags', content_width = 200, content_height = 60, x = 300, y = 300}
             local function item(tag) return {cells = {{text = 'a', x = 1, align = 'left'}}, tag = tag} end
             make_item = item
             tag_list = lists.new{window = tag_window, x = 0, y = 0, width = 200, rows = 3, tags = true, on_select = function() end,
               items = {item('go:location:rowan:1:0')}}
             local ox, oy = tag_window:content_origin()
             note(string.format('tag_point %g %g', ox + 20, oy + 10))",
            // The same row, now given a tag the scene refuses (a space), then a valid one again.
            "tag_list:set_items({make_item('go:location:has space:1:0')})",
            "tag_list:set_items({make_item('go:location:rowan:2:0')})",
        ],
        1800,
        900,
    );
    rig.opened();
    let point: Vec<f64> = note_after(&rig, "tag_point ")
        .split_whitespace()
        .map(|part| part.parse().unwrap())
        .collect();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert_eq!(
        tag_at(&rig, point[0], point[1]).as_deref(),
        Some("go:location:rowan:1:0")
    );
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert_eq!(
        tag_at(&rig, point[0], point[1]),
        None,
        "the refused tag took the row's earlier tag with it"
    );
    rig.opened();
    rig.deliver_fixture_frame_at(1800, 900, 2);
    assert_eq!(
        tag_at(&rig, point[0], point[1]).as_deref(),
        Some("go:location:rowan:2:0"),
        "a valid tag works again"
    );
}
