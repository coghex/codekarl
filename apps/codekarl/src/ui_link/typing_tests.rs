//! The library's typing function (CK-314, `scripts/ui/lib/typing.lua`) in a real UI VM, in the script test rig, with no window: the loop's
//! mapper gathers typed entries, `ck-input`'s snapshot carries them to the link as the input tick does, the stash puts them in the update
//! table, and the script's frame callback runs `typing.dispatch` before `pointer.dispatch`, as `main.lua` does. Widgets of the tests' own
//! record what they are given. Nothing sleeps.

use ck_lua::ConfigValue;
use ck_window::{PointerEvent, TypedEntry};

use super::group_tests::Rig;
use super::widget_witness_tests::points;

/// Two typing widgets (`A` and `B`), each also a pointer widget at a hit region of its own that starts its editing on a press; a third
/// region, `E`, that is "elsewhere" and ends the editing on a press; and the frame callback of `main.lua`. `window.set_text_input` is
/// spied on, and each call is noted with the generation it returned.
const SCRIPT: &str = r#"
local pointer = require("lib.pointer")
local typing = require("lib.typing")
local windows = require("lib.window")
local real_set_text_input = window.set_text_input
window.set_text_input = function(on)
  local generation = real_set_text_input(on)
  window.record("call " .. tostring(on) .. " " .. generation)
  return generation
end
local function entry_text(entry) return entry.kind .. (entry.text ~= nil and (":" .. entry.text) or "") end
local function editor_widget(name)
  local widget = {}
  function widget.on_apply() window.record(name .. " apply") end
  function widget.on_typed(_, entry)
    window.record(name .. " " .. entry_text(entry))
    -- Enter applies the field: its editing ends part way through the update's typing.
    if entry.kind == "enter" then typing.stop(widget) end
    -- Tab applies the field and starts the next editing on the same widget: a new session on the same table.
    if entry.kind == "tab" then typing.stop(widget) typing.start(widget) end
  end
  function widget.on_typed_lost() window.record(name .. " lost") end
  return widget
end
A, B = editor_widget("A"), editor_widget("B")
w = windows.new{title = "typing", x = 100, y = 50, content_width = 300, content_height = 100}
local function region(key, x, widget)
  render.set_hit(key, {{x = x, y = 0, width = 40, height = 40}}, w.content)
  pointer.register(key, {
    on_event = function(_, event)
      if event.kind == "down" then
        window.record("press " .. key)
        if widget ~= nil then typing.start(widget) else typing.stop(typing.editing()) end
      end
    end,
    on_hover = function() end,
    on_overflow = function() end,
  }, w)
end
region("field_a", 0, A)
region("field_b", 60, B)
region("elsewhere", 120, nil)
render.set_hit("capture", {{x = 180, y = 0, width = 40, height = 40}}, w.content)
pointer.register("capture", {
  on_event = function(_, event)
    if event.kind == "down" then
      game.set_modal(true)
      typing.capture("capture-row", function(id) window.record("capture " .. id) end)
      window.record("start_during_capture " .. tostring(typing.start(B)))
    end
  end,
  on_hover = function() end,
  on_overflow = function() end,
}, w)
frame.on_update(function(f)
  typing.dispatch(f)
  if f.capture ~= nil then window.record("capture_result " .. f.capture.id) end
  pointer.dispatch(f.pointer)
  local who = typing.editing() == A and "A" or (typing.editing() == B and "B" or "none")
  window.record("frame epoch=" .. f.typed_epoch .. " typed=" .. #f.typed .. " lost=" .. tostring(f.typed_lost) .. " editing=" .. who)
end)
"#;

fn notes(rig: &Rig) -> Vec<String> {
    rig.tables
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(|value| match value {
            ConfigValue::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The notes recorded since the last call of this function's `mark` (an index into the notes).
fn since(rig: &Rig, from: usize) -> Vec<String> {
    notes(rig)[from..].to_vec()
}

fn field_point(key: &str) -> (f64, f64) {
    // The window is at (100, 50) with a 30-point title and 6 of padding: the content origin is (106, 86).
    let x = match key {
        "field_a" => 0.0,
        "field_b" => 60.0,
        "capture" => 180.0,
        _ => 120.0,
    };
    (106.0 + x + 20.0, 86.0 + 20.0)
}

fn press(key: &str) -> Vec<PointerEvent> {
    let (x, y) = field_point(key);
    vec![
        PointerEvent::Down(points(x, y)),
        PointerEvent::Up(points(x, y)),
    ]
}

fn chars(text: &str) -> Vec<TypedEntry> {
    text.chars().map(TypedEntry::Char).collect()
}

/// One input tick with `typed` then `pointer` handed to the mapper in that order (a mapper keeps the two lists apart, so the order they
/// were fed in is not known to the script), and the update posted for it.
fn update(rig: &mut Rig, typed: &[TypedEntry], pointer: &[PointerEvent]) {
    for entry in typed {
        rig.mapper.typed(*entry);
    }
    rig.feed(pointer);
    rig.deliver_fixture_frame();
}

/// The same, with the pointer fed to the mapper first.
fn update_pointer_first(rig: &mut Rig, pointer: &[PointerEvent], typed: &[TypedEntry]) {
    for event in pointer {
        rig.mapper.pointer(*event);
    }
    for entry in typed {
        rig.mapper.typed(*entry);
    }
    rig.feed(&[]);
    rig.deliver_fixture_frame();
}

fn rig() -> Rig {
    Rig::new_fixture_body(SCRIPT)
}

/// Starts A's editing through a press on its field, and has the "main thread" apply the call (generation 1).
fn editing_a(rig: &mut Rig) {
    update(rig, &[], &press("field_a"));
    rig.mapper.reset_typed(1);
    rig.feed(&[]);
}

#[test]
fn a_widget_gets_the_entries_in_order_while_it_edits_and_none_before_it_starts_or_after_it_ends() {
    let mut rig = rig();
    // Nobody edits: the entries are dropped.
    update(&mut rig, &chars("x"), &[]);
    assert!(!notes(&rig).iter().any(|line| line.starts_with("A ")));
    editing_a(&mut rig);
    assert!(
        notes(&rig).contains(&"call true 1".to_owned()),
        "{:?}",
        notes(&rig)
    );
    let from = notes(&rig).len();
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Backspace,
            TypedEntry::Char('b'),
        ],
        &[],
    );
    let got: Vec<_> = since(&rig, from)
        .into_iter()
        .filter(|line| line.starts_with("A "))
        .collect();
    assert_eq!(got, ["A char:a", "A backspace", "A char:b"]);
    // Ending the editing (a press elsewhere) turns text input off, and the next entries reach no widget.
    update(&mut rig, &[], &press("elsewhere"));
    assert!(
        notes(&rig).contains(&"call false 2".to_owned()),
        "{:?}",
        notes(&rig)
    );
    let from = notes(&rig).len();
    update(&mut rig, &chars("z"), &[]);
    assert!(!since(&rig, from).iter().any(|line| line.starts_with("A ")));
}

#[test]
fn text_input_stays_on_through_the_change_from_one_widget_to_another_and_is_off_when_none_edits() {
    let mut rig = rig();
    update(&mut rig, &[], &press("field_a"));
    update(&mut rig, &[], &press("field_b"));
    update(&mut rig, &[], &press("elsewhere"));
    let calls: Vec<_> = notes(&rig)
        .into_iter()
        .filter(|line| line.starts_with("call "))
        .collect();
    assert_eq!(calls, ["call true 1", "call false 2"]);
    assert!(notes(&rig).iter().any(|line| line.ends_with("editing=B")));
}

#[test]
fn entries_typed_before_the_main_thread_applied_the_call_never_reach_a_widget_that_started_after_them()
 {
    let mut rig = rig();
    update(&mut rig, &[], &press("field_a"));
    // The call (generation 1) is not applied yet: the update's epoch is older, so what it carries was typed under the earlier setting.
    update(&mut rig, &chars("s"), &[]);
    assert!(!notes(&rig).iter().any(|line| line == "A char:s"));
    rig.mapper.reset_typed(1);
    update(&mut rig, &chars("t"), &[]);
    assert!(notes(&rig).iter().any(|line| line == "A char:t"));
    // A widget that ends and another that starts: the entries gathered under the old generations are not B's.
    update(&mut rig, &[], &press("elsewhere"));
    update(&mut rig, &[], &press("field_b"));
    rig.mapper.reset_typed(2);
    update(&mut rig, &chars("u"), &[]);
    assert!(
        !notes(&rig).iter().any(|line| line == "B char:u"),
        "generation 3 is not applied yet"
    );
    rig.mapper.reset_typed(3);
    update(&mut rig, &chars("v"), &[]);
    assert!(notes(&rig).iter().any(|line| line == "B char:v"));
}

#[test]
fn an_enter_that_ends_the_editing_part_way_leaves_the_later_entries_of_the_update_for_no_widget() {
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Enter,
            TypedEntry::Char('b'),
        ],
        &[],
    );
    let got: Vec<_> = since(&rig, from)
        .into_iter()
        .filter(|line| line.starts_with("A ") || line.starts_with("B "))
        .collect();
    assert_eq!(got, ["A char:a", "A enter"]);
}

// ---- Typing and the pointer in one update (rule 7): the typing is handled first, all of it, and then the pointer events ----

#[test]
fn a_key_then_a_click_on_another_widget_gives_the_key_to_the_first_and_the_second_edits_afterwards()
{
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    update(&mut rig, &chars("k"), &press("field_b"));
    let got = since(&rig, from);
    let key = got
        .iter()
        .position(|line| line == "A char:k")
        .expect("A got the key");
    let click = got
        .iter()
        .position(|line| line == "press field_b")
        .expect("the press acted");
    assert!(key < click, "{got:?}");
    assert!(
        got.last().is_some_and(|line| line.ends_with("editing=B")),
        "{got:?}"
    );
}

#[test]
fn a_click_on_another_widget_then_a_key_gives_the_same_result_by_the_rule() {
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    update_pointer_first(&mut rig, &press("field_b"), &chars("k"));
    let got = since(&rig, from);
    let key = got
        .iter()
        .position(|line| line == "A char:k")
        .expect("A got the key");
    let click = got
        .iter()
        .position(|line| line == "press field_b")
        .expect("the press acted");
    assert!(key < click, "{got:?}");
    assert!(!got.iter().any(|line| line == "B char:k"), "{got:?}");
    assert!(
        got.last().is_some_and(|line| line.ends_with("editing=B")),
        "{got:?}"
    );
}

#[test]
fn keys_a_press_elsewhere_and_more_keys_in_one_update_give_the_widget_every_key_before_the_press_which_still_acts()
 {
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    // The mapper holds typed entries and pointer events apart: all three keys are typed whichever side of the press they were on.
    update(&mut rig, &chars("123"), &press("elsewhere"));
    let got = since(&rig, from);
    let keys: Vec<_> = got
        .iter()
        .filter(|line| line.starts_with("A "))
        .cloned()
        .collect();
    assert_eq!(keys, ["A char:1", "A char:2", "A char:3"]);
    let last_key = got.iter().rposition(|line| line.starts_with("A ")).unwrap();
    let click = got
        .iter()
        .position(|line| line == "press elsewhere")
        .expect("the press acted");
    assert!(last_key < click, "{got:?}");
    assert!(
        got.last()
            .is_some_and(|line| line.ends_with("editing=none")),
        "{got:?}"
    );
}

#[test]
fn typing_and_the_pointer_in_two_updates_give_the_order_of_the_updates_in_each_order() {
    // Typing in the first update, the click in the second: A gets the key, then B starts.
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    update(&mut rig, &chars("k"), &[]);
    update(&mut rig, &[], &press("field_b"));
    let got = since(&rig, from);
    assert!(got.iter().any(|line| line == "A char:k"), "{got:?}");
    assert!(!got.iter().any(|line| line == "B char:k"), "{got:?}");
    // The click in the first update, the typing in the second: B is editing, and B gets the key (its call, generation 1, is the
    // latest: there was no second call, since B took the typing from A).
    let mut rig = self::rig();
    editing_a(&mut rig);
    update(&mut rig, &[], &press("field_b"));
    let from = notes(&rig).len();
    update(&mut rig, &chars("k"), &[]);
    let got = since(&rig, from);
    assert!(got.iter().any(|line| line == "B char:k"), "{got:?}");
    assert!(!got.iter().any(|line| line == "A char:k"), "{got:?}");
}

#[test]
fn a_widget_that_ends_and_restarts_its_editing_in_a_handler_gets_a_new_session_and_the_old_updates_tail_is_dropped()
 {
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    // The reviewer's case (CK-314 review 1, F1): `a`, Tab, `b` in one update. Tab ends the editing and starts it again on the same widget,
    // so `b` belongs to the editing that ended and is dropped.
    update(
        &mut rig,
        &[
            TypedEntry::Char('a'),
            TypedEntry::Tab,
            TypedEntry::Char('b'),
        ],
        &[],
    );
    let got: Vec<_> = since(&rig, from)
        .into_iter()
        .filter(|line| line.starts_with("A "))
        .collect();
    assert_eq!(
        got,
        ["A char:a", "A tab"],
        "rule 7: ending the editing drops the rest of this update"
    );
    // The restart moved the generation on twice (off 2, on 3): entries typed under the old epochs do not reach the new session, and fresh
    // ones do once the main thread applied the latest call.
    assert!(
        notes(&rig).contains(&"call false 2".to_owned())
            && notes(&rig).contains(&"call true 3".to_owned())
    );
    update(&mut rig, &chars("x"), &[]);
    assert!(
        !notes(&rig).iter().any(|line| line == "A char:x"),
        "epoch 1 is older than the restart's call"
    );
    rig.mapper.reset_typed(3);
    update(&mut rig, &chars("y"), &[]);
    assert!(
        notes(&rig).iter().any(|line| line == "A char:y"),
        "the restarted session is typed into"
    );
}

#[test]
fn the_loss_notice_of_an_update_does_not_reach_a_session_started_after_it_and_does_reach_the_one_it_belongs_to()
 {
    let mut rig = rig();
    editing_a(&mut rig);
    // A loss reported with entries that end and restart the editing: the notice is for the old session, so no one gets it.
    let from = notes(&rig).len();
    rig.mapper.typed(TypedEntry::Tab);
    rig.mapper.typed_lost();
    rig.feed(&[]);
    rig.deliver_fixture_frame();
    assert!(
        !since(&rig, from).iter().any(|line| line == "A lost"),
        "{:?}",
        since(&rig, from)
    );
    // With no restart the loss reaches the editing it belongs to, after the entries.
    rig.mapper.reset_typed(3);
    let from = notes(&rig).len();
    rig.mapper.typed(TypedEntry::Char('q'));
    rig.mapper.typed_lost();
    rig.feed(&[]);
    rig.deliver_fixture_frame();
    let got: Vec<_> = since(&rig, from)
        .into_iter()
        .filter(|line| line.starts_with("A "))
        .collect();
    assert_eq!(got, ["A char:q", "A lost"]);
}

#[test]
fn starting_again_on_the_widget_already_editing_goes_on_in_the_same_session_and_a_switch_to_another_ends_the_old_one()
 {
    let mut rig = rig();
    editing_a(&mut rig);
    let from = notes(&rig).len();
    // Pressing A's field again while A edits is no restart: no new call, the update's typing is still delivered, press after it.
    update(&mut rig, &chars("k"), &press("field_a"));
    let got = since(&rig, from);
    assert!(got.iter().any(|line| line == "A char:k"), "{got:?}");
    assert!(!got.iter().any(|line| line.starts_with("call ")), "{got:?}");
    // Switching to B ends A's session: A's tail is not B's, and no call is made (text input stays on).
    update(&mut rig, &[], &press("field_b"));
    assert!(
        notes(&rig)
            .last()
            .is_some_and(|line| line.ends_with("editing=B"))
    );
}

#[test]
fn capture_applies_and_ends_the_editor_and_refuses_new_editing_until_its_result() {
    let mut rig = rig();
    editing_a(&mut rig);
    let before_capture = notes(&rig).len();
    update(&mut rig, &[], &press("capture"));
    assert!(
        rig.controls.keys_blocked(),
        "capture can be started while the game's modal key block is active"
    );
    let capture_notes = since(&rig, before_capture);
    assert!(
        capture_notes.contains(&"A apply".to_owned()),
        "{capture_notes:?}"
    );
    assert!(
        capture_notes.contains(&"call false 2".to_owned()),
        "{capture_notes:?}"
    );
    assert!(
        capture_notes.contains(&"capture capture-row".to_owned()),
        "{capture_notes:?}"
    );
    assert!(
        capture_notes.contains(&"start_during_capture false".to_owned()),
        "a widget cannot start a typing session while a capture is waiting: {capture_notes:?}"
    );

    let before_typed = notes(&rig).len();
    update(&mut rig, &chars("q"), &[]);
    let typed_notes = since(&rig, before_typed);
    assert!(
        !typed_notes
            .iter()
            .any(|line| line == "A char:q" || line == "B char:q"),
        "the capture and typed-entry route never share a key: {typed_notes:?}"
    );

    rig.mapper.replace_bindings(
        ck_input::Bindings::from_entries([
            ("close".to_owned(), vec!["Escape".to_owned()]),
            ("time_pause".to_owned(), vec!["Escape".to_owned()]),
        ])
        .expect("close and a modal game action share the captured key"),
    );
    rig.mapper.arm_capture("capture-row".to_owned());
    rig.mapper.key(ck_input::Key::Escape, true, false);
    let captured_input = rig.mapper.snapshot();
    assert!(
        captured_input.capture.is_some(),
        "the modal key was captured"
    );
    assert!(
        captured_input.transitions.is_empty(),
        "capture adds neither a blocked game action nor a close transition: {:?}",
        captured_input.transitions
    );
    assert_eq!(
        rig.mapper.take_close_presses(),
        0,
        "capture does not close the window"
    );
    rig.feed(&[]);
    rig.deliver_fixture_frame();
    assert!(
        notes(&rig)
            .iter()
            .any(|line| line == "capture_result capture-row"),
        "the result ends the library's capture guard"
    );

    let before_next_editor = notes(&rig).len();
    update(&mut rig, &[], &press("field_b"));
    rig.mapper.reset_typed(3);
    update(&mut rig, &chars("z"), &[]);
    let next_notes = since(&rig, before_next_editor);
    assert!(
        next_notes.contains(&"B char:z".to_owned()),
        "{next_notes:?}"
    );
}

#[test]
fn capture_without_an_editor_still_blocks_a_new_typing_session_until_its_result() {
    let mut rig = rig();
    let before = notes(&rig).len();
    update(&mut rig, &[], &press("capture"));
    let captured = since(&rig, before);
    assert!(
        captured.contains(&"capture capture-row".to_owned()),
        "{captured:?}"
    );
    assert!(
        captured.contains(&"start_during_capture false".to_owned()),
        "the capture guard also applies when there was no editor: {captured:?}"
    );
    assert!(
        !captured.iter().any(|line| line.ends_with(" apply")),
        "{captured:?}"
    );
    let before_typed = notes(&rig).len();
    update(&mut rig, &chars("q"), &[]);
    let typed = since(&rig, before_typed);
    assert!(
        !typed
            .iter()
            .any(|line| line == "A char:q" || line == "B char:q"),
        "no editor can receive a character while capture waits: {typed:?}"
    );
}
