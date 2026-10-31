//! Normalised keys and focus, side-aware modifiers and the bounded recording with its reconciliation (CK-121), all
//! driven by **marked** synthetic events through the real dispatch (`sendEvent:`), the content view's responder
//! methods and the pump. `drain_events` dispatches what is queued without handing anything over, so a case can hold the
//! pump until the point it states. These are cases of `window_cocoa.rs` (they run in its worker processes, quiet and
//! with the owner typing).

use std::sync::OnceLock;
use std::time::Duration;

use ck_input::{Bindings, Key, KeyStates, Mapper};
use ck_window::{NativeEvent, Wait};

use crate::support::{self, KeyboardEvent, Pumped, Scheduled};
use crate::{ESCAPE, Fixture};

/// The family bits of `NSEvent.modifierFlags` (`NSEventModifierFlagShift`, `Control`, `Option`, `Command`).
const SHIFT: u64 = 1 << 17;
const CONTROL: u64 = 1 << 18;
const OPTION: u64 = 1 << 19;
const COMMAND: u64 = 1 << 20;
/// The device-dependent side bits (`IOKit`'s `hidsystem/IOLLEvent.h`, `NX_DEVICE*KEYMASK`).
const L_CONTROL: u64 = 0x0000_0001;
const L_SHIFT: u64 = 0x0000_0002;
const R_SHIFT: u64 = 0x0000_0004;
const L_COMMAND: u64 = 0x0000_0008;
const R_COMMAND: u64 = 0x0000_0010;
const L_OPTION: u64 = 0x0000_0020;
const R_OPTION: u64 = 0x0000_0040;
const R_CONTROL: u64 = 0x0000_2000;

/// One modifier family: its two keys, its family bit and its two side bits.
struct Family {
    name: &'static str,
    left: Key,
    right: Key,
    aggregate: u64,
    left_bit: u64,
    right_bit: u64,
}

const FAMILIES: [Family; 4] = [
    Family {
        name: "Shift",
        left: Key::ShiftLeft,
        right: Key::ShiftRight,
        aggregate: SHIFT,
        left_bit: L_SHIFT,
        right_bit: R_SHIFT,
    },
    Family {
        name: "Control",
        left: Key::ControlLeft,
        right: Key::ControlRight,
        aggregate: CONTROL,
        left_bit: L_CONTROL,
        right_bit: R_CONTROL,
    },
    Family {
        name: "Option",
        left: Key::AltLeft,
        right: Key::AltRight,
        aggregate: OPTION,
        left_bit: L_OPTION,
        right_bit: R_OPTION,
    },
    Family {
        name: "Command",
        left: Key::MetaLeft,
        right: Key::MetaRight,
        aggregate: COMMAND,
        left_bit: L_COMMAND,
        right_bit: R_COMMAND,
    },
];

/// The macOS virtual key code of `key`, from the native table.
fn code(key: Key) -> u16 {
    static TABLE: OnceLock<Vec<(u16, u8, String)>> = OnceLock::new();
    TABLE
        .get_or_init(support::key_table)
        .iter()
        .find(|(_, _, name)| name == key.name())
        .map_or_else(
            || panic!("{} is not in the key table", key.name()),
            |(code, _, _)| *code,
        )
}

fn down(key: Key) {
    assert!(support::post_key(code(key), 0, false, ""), "{key:?}");
}

fn up(key: Key) {
    assert!(
        support::post_keyboard_event(KeyboardEvent::KeyUp, code(key), 0),
        "{key:?}"
    );
}

fn modifier(key: Key, flags: u64) {
    assert!(
        support::post_keyboard_event(KeyboardEvent::ModifierChange, code(key), flags),
        "{key:?}"
    );
}

/// The ordinary way: dispatch everything queued, then pump (the cap is the loop's default, 64).
fn deliver(fixture: &mut Fixture) -> Pumped {
    fixture.drain()
}

/// Dispatches everything queued and **does not hand it over**: it is recorded natively and waits for the next pump.
fn record_only() {
    support::drain_events();
}

/// One pump with a cap large enough for anything the ring holds.
fn pump_all(fixture: &mut Fixture) -> Pumped {
    fixture.pump(Wait::Poll, 1000)
}

/// 150 key-down / key-up pairs of an unrelated key: 300 transitions, enough to overflow the ring of 256.
fn fill(fixture: &Fixture) {
    let _ = fixture;
    assert!(support::record_keys(150, code(Key::KeyZ)));
}

fn only_resync(pumped: &Pumped) -> KeyStates {
    match pumped.input.as_slice() {
        [NativeEvent::KeyResync(states)] => *states,
        other => panic!("exactly one resync was expected, not {other:?}"),
    }
}

/// Feeds what a pump handed over to a mapper, as the loop does.
fn feed(mapper: &mut Mapper, pumped: &Pumped) {
    for event in &pumped.input {
        match event {
            NativeEvent::Key { key, down, repeat } => mapper.key(*key, *down, *repeat),
            NativeEvent::KeyResync(states) => mapper.resync(states),
            NativeEvent::FocusLost => mapper.focus_lost(),
            _ => {}
        }
    }
}

fn bindings(entries: &[(&str, &[&str])]) -> Bindings {
    Bindings::from_entries(entries.iter().map(|(action, keys)| {
        (
            (*action).to_owned(),
            keys.iter().map(|key| (*key).to_owned()).collect(),
        )
    }))
    .expect("valid bindings")
}

/// The default bindings plus the actions the schedules look at.
fn watching() -> Mapper {
    Mapper::new(bindings(&[
        ("chord", &["Control+KeyR"]),
        ("control", &["ControlLeft"]),
        ("jay", &["KeyJ"]),
        ("aitch", &["KeyH"]),
    ]))
}

/// The state of the named action in a snapshot: `(held, pressed, released)`.
fn state(mapper: &mut Mapper, name: &str) -> (bool, bool, bool) {
    let snapshot = mapper.snapshot();
    let action = snapshot
        .actions
        .iter()
        .find(|action| action.name == name)
        .unwrap_or_else(|| panic!("no action {name}"));
    (action.held, action.pressed, action.released)
}

// ---------------------------------------------------------------------------------------------
// The table and the normalisation
// ---------------------------------------------------------------------------------------------

/// The native table agrees with ck-input: 81 entries, one per key and one per code, each key's number its place in
/// `Key::ALL` and its name `Key::name`; the modifiers are the codes the specification names.
pub(crate) fn key_table_matches_ck_input() {
    let table = support::key_table();
    assert_eq!(table.len(), ck_input::KEY_COUNT);
    let mut codes: Vec<u16> = table.iter().map(|(code, _, _)| *code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), ck_input::KEY_COUNT, "one code per key");
    let mut numbers: Vec<u8> = table.iter().map(|(_, key, _)| *key).collect();
    numbers.sort_unstable();
    assert_eq!(
        numbers,
        (0..u8::try_from(ck_input::KEY_COUNT).expect("81")).collect::<Vec<_>>(),
        "one number per key"
    );
    for (code, number, name) in &table {
        let key = Key::ALL[usize::from(*number)];
        assert_eq!(key.name(), name, "code {code}: number {number}");
        assert_eq!(Key::from_name(name), Some(key));
    }
    for (expected, key) in [
        (56, Key::ShiftLeft),
        (60, Key::ShiftRight),
        (59, Key::ControlLeft),
        (62, Key::ControlRight),
        (58, Key::AltLeft),
        (61, Key::AltRight),
        (55, Key::MetaLeft),
        (54, Key::MetaRight),
        (53, Key::Escape),
        (0, Key::KeyA),
        (36, Key::Enter),
    ] {
        assert_eq!(code(key), expected, "{key:?}");
    }
}

/// Every table entry produces its `Key` (down, repeat, up); a modifier's entries are the side-aware cases below.
pub(crate) fn every_table_key_produces_its_key() {
    let mut fixture = Fixture::open(300, 200);
    let modifiers = [54, 55, 56, 58, 59, 60, 61, 62];
    for (code, number, _) in support::key_table() {
        if modifiers.contains(&code) {
            continue;
        }
        let key = Key::ALL[usize::from(number)];
        assert!(support::post_key(code, 0, false, ""));
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(key, true, false)],
            "{key:?} down"
        );
        assert!(support::post_key(code, 0, true, ""));
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(key, true, true)],
            "{key:?} repeat"
        );
        up(key);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(key, false, false)],
            "{key:?} up"
        );
    }
    assert!(!support::native_keys().any_held());
}

/// A code outside the table produces nothing, and none of it reaches the beep path or leaves anything held.
pub(crate) fn unmapped_codes_produce_nothing_and_no_beep() {
    let mut fixture = Fixture::open(300, 200);
    let beeps = support::no_responder_count();
    let seen = support::key_downs_seen();
    // Keypad Enter (76), a code past the table (127) and Caps Lock (57, a modifier-change code the set does not have).
    assert!(support::post_key(76, 0, false, "\r"));
    assert!(support::post_key(127, 0, false, "x"));
    assert!(support::post_keyboard_event(KeyboardEvent::KeyUp, 76, 0));
    assert!(support::post_keyboard_event(
        KeyboardEvent::ModifierChange,
        57,
        1 << 16
    ));
    let pumped = deliver(&mut fixture);
    assert!(pumped.input.is_empty(), "{pumped:?}");
    assert_eq!(
        support::key_downs_seen(),
        seen + 2,
        "the two key-downs reached the view"
    );
    assert_eq!(support::no_responder_count(), beeps, "no beep");
    assert!(!support::native_keys().any_held());
}

/// Every modifier, both sides: down and up are the side's own `Key`. The sequences are the side bits' job: left
/// released while right stays held, right released, and the reverse order.
pub(crate) fn modifiers_are_side_aware() {
    let mut fixture = Fixture::open(300, 200);
    for f in &FAMILIES {
        let name = f.name;
        // Left down, right down, left released (right still held), right released.
        modifier(f.left, f.aggregate | f.left_bit);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, true, false)],
            "{name} left down"
        );
        modifier(f.right, f.aggregate | f.left_bit | f.right_bit);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.right, true, false)],
            "{name} right down"
        );
        modifier(f.left, f.aggregate | f.right_bit);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, false, false)],
            "{name} left up"
        );
        let native = support::native_keys();
        assert!(
            native.is_held(f.right) && !native.is_held(f.left),
            "{name}: right stays held"
        );
        modifier(f.right, 0);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.right, false, false)],
            "{name} right up"
        );
        assert!(!support::native_keys().any_held(), "{name}");
        // The reverse order.
        modifier(f.right, f.aggregate | f.right_bit);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.right, true, false)],
            "{name} right down"
        );
        modifier(f.left, f.aggregate | f.left_bit | f.right_bit);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, true, false)],
            "{name} left down"
        );
        modifier(f.right, f.aggregate | f.left_bit);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.right, false, false)],
            "{name} right up"
        );
        let native = support::native_keys();
        assert!(
            native.is_held(f.left) && !native.is_held(f.right),
            "{name}: left stays held"
        );
        modifier(f.left, 0);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, false, false)],
            "{name} left up"
        );
        assert!(!support::native_keys().any_held(), "{name}");
    }
}

/// A mixed-side prefixed binding: `ControlLeft` held, then `ControlRight` held, left released: `Control+KeyR` stays
/// held through the right one. (Deciding from the aggregate Control bit would release it at the left's release.)
pub(crate) fn a_mixed_side_chord_stays_held() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    modifier(Key::ControlLeft, CONTROL | L_CONTROL);
    modifier(Key::ControlRight, CONTROL | L_CONTROL | R_CONTROL);
    modifier(Key::ControlLeft, CONTROL | R_CONTROL);
    down(Key::KeyR);
    let pumped = deliver(&mut fixture);
    assert_eq!(
        pumped.keys(),
        [
            (Key::ControlLeft, true, false),
            (Key::ControlRight, true, false),
            (Key::ControlLeft, false, false),
            (Key::KeyR, true, false)
        ]
    );
    feed(&mut mapper, &pumped);
    assert!(
        state(&mut mapper, "chord").0,
        "Control+KeyR is held by the right Control"
    );
    modifier(Key::ControlRight, 0);
    feed(&mut mapper, &deliver(&mut fixture));
    assert!(!state(&mut mapper, "chord").0);
    up(Key::KeyR);
    let _ = deliver(&mut fixture);
}

/// With no side bits in the event (some virtual or remote input) the code toggles the side's state, and whenever the
/// family's own bit is clear both sides are released.
pub(crate) fn modifiers_without_side_bits_toggle_and_release_with_the_family() {
    let mut fixture = Fixture::open(300, 200);
    for f in &FAMILIES {
        let name = f.name;
        modifier(f.left, f.aggregate);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, true, false)],
            "{name}"
        );
        modifier(f.right, f.aggregate);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.right, true, false)],
            "{name}"
        );
        // Left toggles off while the family bit is still set (right is held).
        modifier(f.left, f.aggregate);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, false, false)],
            "{name}"
        );
        assert!(support::native_keys().is_held(f.right), "{name}");
        // The family bit clear: right is released.
        modifier(f.right, 0);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.right, false, false)],
            "{name}"
        );
        assert!(!support::native_keys().any_held(), "{name}");
        // Both held, then a release event whose family bit is clear releases both, whichever side it names.
        modifier(f.left, f.aggregate);
        modifier(f.right, f.aggregate);
        let _ = deliver(&mut fixture);
        modifier(f.left, 0);
        assert_eq!(
            deliver(&mut fixture).keys(),
            [(f.left, false, false), (f.right, false, false)],
            "{name}: a missed release can never outlive the family's own"
        );
        assert!(!support::native_keys().any_held(), "{name}");
    }
}

// ---------------------------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------------------------

/// Resign and become key reach the delegate and are delivered as `FocusLost` / `FocusGained`; a loss clears every
/// native held key and is delivered before anything recorded after it.
pub(crate) fn focus_events_and_the_reset() {
    let mut fixture = Fixture::open(300, 200);
    support::resign_key();
    assert!(support::native_keys().focus_lost_pending);
    assert_eq!(deliver(&mut fixture).input, [NativeEvent::FocusLost]);
    assert!(!support::native_keys().focus_lost_pending, "delivered once");
    assert!(deliver(&mut fixture).input.is_empty());
    support::become_key();
    assert_eq!(deliver(&mut fixture).input, [NativeEvent::FocusGained]);

    // Modifiers and a key held: the loss clears the native state and the loop's mapper alike.
    let mut mapper = watching();
    modifier(Key::ControlLeft, CONTROL | L_CONTROL);
    down(Key::KeyJ);
    feed(&mut mapper, &deliver(&mut fixture));
    assert!(state(&mut mapper, "control").0 && state(&mut mapper, "jay").0);
    assert!(support::native_keys().any_held());
    support::resign_key();
    assert!(
        !support::native_keys().any_held(),
        "the native held bitset was cleared"
    );
    // A key recorded after the loss is delivered after it.
    down(Key::KeyH);
    record_only();
    let pumped = pump_all(&mut fixture);
    assert_eq!(
        pumped.input,
        [
            NativeEvent::FocusLost,
            NativeEvent::Key {
                key: Key::KeyH,
                down: true,
                repeat: false
            }
        ]
    );
    feed(&mut mapper, &pumped);
    assert!(!state(&mut mapper, "control").0 && !state(&mut mapper, "jay").0);
    assert!(
        state(&mut mapper, "aitch").0,
        "the key after the loss is a new press"
    );
    up(Key::KeyH);
    let _ = deliver(&mut fixture);
}

// ---------------------------------------------------------------------------------------------
// Bounded recording and reconciliation
// ---------------------------------------------------------------------------------------------

/// 300 transitions before one pump: no ordered transition of the span is delivered, one resync, the overflow
/// reported, and the mapper's final held state equals the native one - a key still physically held stays held, a key
/// released during the overflow is released.
pub(crate) fn bounded_recording_resyncs_to_the_native_state() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    down(Key::KeyJ);
    down(Key::KeyH);
    let pumped = deliver(&mut fixture);
    feed(&mut mapper, &pumped);
    assert!(state(&mut mapper, "jay").0 && state(&mut mapper, "aitch").0);
    // J is released inside the overflow, H stays down; 300 marked transitions of an unrelated key.
    up(Key::KeyJ);
    for _ in 0..150 {
        down(Key::KeyZ);
        up(Key::KeyZ);
    }
    record_only();
    let native = support::native_keys();
    assert!(native.overflow && native.ring_length == 256, "{native:?}");
    assert!(
        native.pressed_latched(Key::KeyZ) && native.released_latched(Key::KeyZ),
        "the discarded transitions are in the latches: {native:?}"
    );
    assert!(
        !native.pressed_latched(Key::KeyJ) && native.released_latched(Key::KeyJ),
        "J's release was the oldest transition and was discarded: {native:?}"
    );
    let total_before = pumped.overflow_total;
    let pumped = pump_all(&mut fixture);
    let states = only_resync(&pumped);
    assert!(pumped.overflow_total >= total_before + 45, "{pumped:?}");
    assert!(states.is_held(Key::KeyH) && !states.is_held(Key::KeyJ) && !states.is_held(Key::KeyZ));
    assert!(states.was_pressed(Key::KeyZ) && states.was_released(Key::KeyZ));
    feed(&mut mapper, &pumped);
    assert!(
        state(&mut mapper, "aitch").0,
        "still physically held: stays held"
    );
    assert!(
        !state(&mut mapper, "jay").0,
        "released during the overflow: released"
    );
    assert!(mapper.snapshot().overflow);
    let native = support::native_keys();
    assert!(
        native.is_held(Key::KeyH) && !native.is_held(Key::KeyJ),
        "the native held state is what the resync reported"
    );
    assert!(
        native.latches_clear() && !native.overflow && native.ring_length == 0,
        "{native:?}"
    );
    assert!(
        deliver(&mut fixture).input.is_empty(),
        "nothing of the span comes again"
    );
    up(Key::KeyH);
    let _ = deliver(&mut fixture);
}

/// An Escape tap entirely in the discarded span, then unrelated transitions: one close press reaches the mapper and
/// the overflow is reported; a non-close tap there latches `pressed` and `released` for its action.
pub(crate) fn a_tap_in_the_discarded_span_is_latched_and_escape_still_closes() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    down(Key::Escape);
    up(Key::Escape);
    record_only();
    fill(&fixture);
    let pumped = pump_all(&mut fixture);
    only_resync(&pumped);
    feed(&mut mapper, &pumped);
    assert_eq!(
        mapper.take_close_presses(),
        1,
        "a complete Escape tap still requests a close"
    );
    let snapshot = mapper.snapshot();
    assert!(snapshot.overflow, "{snapshot:?}");
    // A non-close tap.
    down(Key::KeyR);
    up(Key::KeyR);
    record_only();
    fill(&fixture);
    let pumped = pump_all(&mut fixture);
    only_resync(&pumped);
    feed(&mut mapper, &pumped);
    assert_eq!(mapper.take_close_presses(), 0);
    let (held, pressed, released) = state(&mut mapper, "reload_texture");
    assert!(
        !held && pressed && released,
        "reload_texture latched: {pressed} {released}"
    );
}

/// Which schedule of the reconciliation a case runs.
#[derive(Clone, Copy)]
enum Schedule {
    /// `ControlLeft` held and delivered; its release, then enough transitions to overflow, then a `KeyR` tap that
    /// would survive in the ring.
    StaleModifier,
    /// `KeyR` down and up, then `ControlLeft` down and held, all in the discarded span.
    OrderUnknown,
}

/// Runs a schedule against `mapper` and returns what the pump handed over (one resync).
fn run_schedule(fixture: &mut Fixture, mapper: &mut Mapper, schedule: Schedule) -> Pumped {
    match schedule {
        Schedule::StaleModifier => {
            modifier(Key::ControlLeft, CONTROL | L_CONTROL);
            feed(mapper, &deliver(fixture));
            modifier(Key::ControlLeft, 0);
            record_only();
            fill(fixture);
            down(Key::KeyR);
            up(Key::KeyR);
            record_only();
        }
        Schedule::OrderUnknown => {
            down(Key::KeyR);
            up(Key::KeyR);
            modifier(Key::ControlLeft, CONTROL | L_CONTROL);
            record_only();
            fill(fixture);
        }
    }
    let pumped = pump_all(fixture);
    only_resync(&pumped);
    feed(mapper, &pumped);
    pumped
}

/// Schedule A: the surviving `KeyR` tap is not mapped against the stale `Control` (r2 would press `Control+KeyR`);
/// `reload_texture` (`KeyR`) is latched; nothing is held afterwards.
pub(crate) fn schedule_a_a_stale_modifier_is_not_replayed() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    let _ = run_schedule(&mut fixture, &mut mapper, Schedule::StaleModifier);
    let chord = state(&mut mapper, "chord");
    assert_eq!(chord, (false, false, false), "Control+KeyR is not pressed");
    let (held, pressed, released) = state(&mut mapper, "reload_texture");
    assert!(!held && pressed && released, "KeyR is latched");
    assert!(!state(&mut mapper, "control").0, "ControlLeft was released");
    assert!(
        !support::native_keys().any_held(),
        "nothing is held afterwards"
    );
}

/// Schedule B: the order is unknown, so no chord is invented: `Control+KeyR` is not pressed, `reload_texture` is
/// latched and `ControlLeft` is held (r2's prefix rule would press the chord).
pub(crate) fn schedule_b_an_unknown_order_invents_no_chord() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    let _ = run_schedule(&mut fixture, &mut mapper, Schedule::OrderUnknown);
    assert_eq!(state(&mut mapper, "chord"), (false, false, false));
    let (held, pressed, released) = state(&mut mapper, "reload_texture");
    assert!(!held && pressed && released);
    assert!(state(&mut mapper, "control").0, "ControlLeft is held");
    assert!(support::native_keys().is_held(Key::ControlLeft));
    modifier(Key::ControlLeft, 0);
    let _ = deliver(&mut fixture);
}

/// Both schedules with `close = ["Control+KeyR"]`: no close request; with both keys physically held at the resync:
/// one close.
pub(crate) fn a_prefixed_close_binding_closes_only_through_a_chord_held_now() {
    let mut fixture = Fixture::open(300, 200);
    let chord_close = || Mapper::new(bindings(&[("close", &["Control+KeyR"])]));
    for schedule in [Schedule::StaleModifier, Schedule::OrderUnknown] {
        let mut mapper = chord_close();
        let _ = run_schedule(&mut fixture, &mut mapper, schedule);
        assert_eq!(
            mapper.take_close_presses(),
            0,
            "no close from a chord that was not proven"
        );
        modifier(Key::ControlLeft, 0);
        let _ = deliver(&mut fixture);
    }
    // Both physically held at the resync: the chord overlaps now.
    let mut mapper = chord_close();
    fill(&fixture);
    modifier(Key::ControlLeft, CONTROL | L_CONTROL);
    down(Key::KeyR);
    record_only();
    let pumped = pump_all(&mut fixture);
    only_resync(&pumped);
    feed(&mut mapper, &pumped);
    assert_eq!(mapper.take_close_presses(), 1, "one close");
    up(Key::KeyR);
    modifier(Key::ControlLeft, 0);
    let _ = deliver(&mut fixture);
}

/// Exactly once: a tap delivered by an ordinary pump is one close, and an overflow span without it does not close
/// again; the latches clear at the resync; after a resync an ordinary pump delivers nothing of the span.
pub(crate) fn every_transition_is_delivered_or_latched_exactly_once() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    down(Key::Escape);
    up(Key::Escape);
    let ordinary = deliver(&mut fixture);
    assert_eq!(
        ordinary.keys(),
        [(Key::Escape, true, false), (Key::Escape, false, false)]
    );
    feed(&mut mapper, &ordinary);
    assert_eq!(mapper.take_close_presses(), 1);
    // An overflow span with no Escape.
    fill(&fixture);
    let pumped = pump_all(&mut fixture);
    let states = only_resync(&pumped);
    assert!(!states.was_pressed(Key::Escape) && !states.was_released(Key::Escape));
    feed(&mut mapper, &pumped);
    assert_eq!(mapper.take_close_presses(), 0, "no second close");
    assert!(
        support::native_keys().latches_clear(),
        "Escape's latches, like every other, are clear"
    );
    // An ordinary pump after the resync: only what is recorded now.
    down(Key::KeyR);
    up(Key::KeyR);
    assert_eq!(
        deliver(&mut fixture).keys(),
        [(Key::KeyR, true, false), (Key::KeyR, false, false)],
        "nothing from the span appears again"
    );
    assert!(deliver(&mut fixture).input.is_empty());
}

/// A focus loss with a pending overflow: nothing held afterwards, `FocusLost` delivered first and alone, no resync for
/// keys from before the reset, and nothing resurrected.
pub(crate) fn focus_loss_with_a_pending_overflow_delivers_no_resync() {
    let mut fixture = Fixture::open(300, 200);
    down(Key::KeyJ);
    record_only();
    fill(&fixture);
    assert!(support::native_keys().overflow);
    support::resign_key();
    let native = support::native_keys();
    assert!(
        !native.any_held() && !native.overflow && native.ring_length == 0 && native.latches_clear()
    );
    let pumped = pump_all(&mut fixture);
    assert_eq!(pumped.input, [NativeEvent::FocusLost], "{pumped:?}");
    assert!(
        deliver(&mut fixture).input.is_empty(),
        "nothing is resurrected after it"
    );
}

/// An ordinary backlog is delivered in order over capped pumps without latching; an overflow during the backlog
/// discards the undelivered remainder into one whole resync.
pub(crate) fn recovery_over_capped_pumps() {
    let mut fixture = Fixture::open(300, 200);
    // 100 down / up pairs of one key recorded (200 transitions, no overflow), then pumps of cap 64.
    for _ in 0..100 {
        down(Key::KeyZ);
        up(Key::KeyZ);
    }
    record_only();
    let native = support::native_keys();
    assert!(!native.overflow && native.ring_length == 200, "{native:?}");
    let expected = |at: usize| (Key::KeyZ, at.is_multiple_of(2), false);
    let mut seen = Vec::new();
    for want in [64, 64, 64, 8] {
        let pumped = fixture.pump(Wait::Poll, 64);
        assert_eq!(pumped.keys().len(), want, "{pumped:?}");
        seen.extend(pumped.keys());
    }
    assert_eq!(seen.len(), 200);
    for (at, key) in seen.iter().enumerate() {
        assert_eq!(*key, expected(at), "delivered in order, #{at}");
    }
    assert!(
        support::native_keys().latches_clear(),
        "an ordinary backlog is not latched"
    );
    // The same backlog, with an overflow while 136 of it are still undelivered.
    for _ in 0..100 {
        down(Key::KeyZ);
        up(Key::KeyZ);
    }
    record_only();
    let first = fixture.pump(Wait::Poll, 64);
    assert_eq!(first.keys().len(), 64);
    assert_eq!(support::native_keys().ring_length, 136);
    fill(&fixture);
    let pumped = fixture.pump(Wait::Poll, 64);
    only_resync(&pumped);
    assert!(
        pumped.keys().is_empty(),
        "the undelivered remainder is not delivered in order"
    );
    assert!(fixture.pump(Wait::Poll, 64).input.is_empty());
    assert!(support::native_keys().latches_clear());
}

// ---------------------------------------------------------------------------------------------
// Typing
// ---------------------------------------------------------------------------------------------

/// The owner's typing (unmarked key-downs, repeats, key-ups, modifier changes, Escape among them) reaches nothing: the
/// filter's counter rises, no `Key` event, held key, close or beep appears, with or without the typist thread.
pub(crate) fn typing_reaches_no_window() {
    let mut fixture = Fixture::open(300, 200);
    let dropped = support::filtered_keys();
    let beeps = support::no_responder_count();
    let seen = support::key_downs_seen();
    assert!(support::post_unmarked_key(ESCAPE, 0, false, "\u{1b}"));
    assert!(support::post_unmarked_key(ESCAPE, 0, true, "\u{1b}"));
    assert!(support::post_unmarked_keyboard_event(
        KeyboardEvent::KeyUp,
        ESCAPE,
        0
    ));
    assert!(support::post_unmarked_keyboard_event(
        KeyboardEvent::ModifierChange,
        code(Key::ControlLeft),
        CONTROL | L_CONTROL
    ));
    assert!(support::post_unmarked_keyboard_event(
        KeyboardEvent::ModifierChange,
        code(Key::ShiftRight),
        SHIFT | R_SHIFT
    ));
    let mut total = Pumped::default();
    let until =
        std::time::Instant::now() + Duration::from_millis(if crate::typing() { 400 } else { 50 });
    while std::time::Instant::now() < until {
        total.merge(deliver(&mut fixture));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        total.input.is_empty(),
        "no Key or focus event from typing: {total:?}"
    );
    assert_eq!(total.close_source, 0);
    assert!(!support::native_keys().any_held());
    assert!(
        support::filtered_keys() >= dropped + 5,
        "the filter dropped them"
    );
    assert_eq!(
        support::key_downs_seen(),
        seen,
        "none reached the content view"
    );
    assert_eq!(support::no_responder_count(), beeps, "no beep");
}

/// Recorded keys are pending state: with no native event queued, a pump that is asked to wait returns at once with
/// them (the wake of the recording and every incidental event are taken off the queue, so only the pending-state
/// branch can end the wait), and a focus loss, which is not a ring entry, counts too.
pub(crate) fn recorded_keys_and_focus_do_not_let_a_pump_sleep() {
    let mut fixture = Fixture::open(300, 200);
    for focus in [false, true] {
        if focus {
            support::resign_key();
        } else {
            assert!(support::record_keys(1, code(Key::KeyZ)));
        }
        assert!(
            support::discard_wake_events() >= 1,
            "the recording posted a wake, which the test removes to test the pending-state branch"
        );
        for _ in 0..3 {
            support::drain_events();
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = support::discard_wake_events();
        support::post_marker_after(Duration::from_secs(4), 9);
        let marker0 = support::markers_dispatched();
        let started = std::time::Instant::now();
        let pumped = fixture.pump(Wait::UntilEvent, 64);
        assert!(!pumped.input.is_empty(), "{pumped:?}");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "a pump with recorded keys or focus does not sleep ({:?})",
            started.elapsed()
        );
        assert_eq!(support::markers_dispatched(), marker0);
        let _ = fixture.drain();
    }
}

/// An auto-repeat is no press, also when it is the transition that was discarded: 300 repeats of a key already held
/// overflow the ring, and the resync neither reports a press of it nor makes the mapper see a new one.
pub(crate) fn discarded_repeats_are_no_presses() {
    let mut fixture = Fixture::open(300, 200);
    let mut mapper = watching();
    down(Key::KeyJ);
    feed(&mut mapper, &deliver(&mut fixture));
    let snapshot = mapper.snapshot();
    mapper.acknowledge(snapshot.sequence);
    assert_eq!(state(&mut mapper, "jay"), (true, false, false));
    for _ in 0..300 {
        assert!(support::post_key(code(Key::KeyJ), 0, true, ""));
    }
    record_only();
    assert!(support::native_keys().overflow);
    let pumped = pump_all(&mut fixture);
    let states = only_resync(&pumped);
    assert!(states.is_held(Key::KeyJ), "still held");
    assert!(
        !states.was_pressed(Key::KeyJ),
        "a discarded repeat is no press"
    );
    feed(&mut mapper, &pumped);
    assert_eq!(
        state(&mut mapper, "jay"),
        (true, false, false),
        "held, and no new press for the mapper"
    );
    up(Key::KeyJ);
    let _ = deliver(&mut fixture);
}

/// What a pump handed over, as short words: `KeyA+` / `KeyA-` for a key down / up, `Lost`, `Gained`, `Resync`.
fn trace(pumped: &Pumped) -> Vec<String> {
    pumped
        .input
        .iter()
        .map(|event| match event {
            NativeEvent::Key { key, down, .. } => {
                format!("{key:?}{}", if *down { "+" } else { "-" })
            }
            NativeEvent::FocusLost => "Lost".to_owned(),
            NativeEvent::FocusGained => "Gained".to_owned(),
            NativeEvent::KeyResync(_) => "Resync".to_owned(),
            other => format!("{other:?}"),
        })
        .collect()
}

/// A fixture with nothing pending (a gain from opening the window, owner typing already dropped).
fn settled() -> Fixture {
    let mut fixture = Fixture::open(300, 200);
    let _ = deliver(&mut fixture);
    let _ = pump_all(&mut fixture);
    fixture
}

fn pump_cap(fixture: &mut Fixture, cap: usize) -> Vec<String> {
    trace(&fixture.pump(Wait::Poll, cap))
}

/// Input recorded by a callback while a pump dispatches belongs to the next pump: with an entry backlog (even a
/// capped one) the pump hands over only what was recorded before it, in order, and what dispatch recorded - keys, a
/// focus loss, an overflow - stays pending and follows.
pub(crate) fn input_recorded_while_a_pump_dispatches_waits_for_the_next_pump() {
    let mut fixture = settled();
    let dispatched = support::markers_dispatched();

    // Keys: A before the pump, B recorded by a timer inside it.
    assert!(support::record_keys(1, code(Key::KeyA)));
    support::post_recording(Scheduled::KeyPairs(1, code(Key::KeyB)));
    assert_eq!(pump_cap(&mut fixture, 1000), ["KeyA+", "KeyA-"]);
    assert_eq!(
        support::markers_dispatched(),
        dispatched + 1,
        "the recording event was dispatched inside the pump"
    );
    assert_eq!(support::native_keys().ring_length, 2, "B waits, recorded");
    assert!(support::native_keys().any_held() || support::native_keys().ring_length == 2);
    assert_eq!(pump_cap(&mut fixture, 1000), ["KeyB+", "KeyB-"]);
    assert!(pump_cap(&mut fixture, 1000).is_empty());

    // A capped backlog with new input arriving during the pump: order is kept across the pumps.
    assert!(support::record_keys(3, code(Key::KeyZ)));
    support::post_recording(Scheduled::KeyPairs(1, code(Key::KeyB)));
    assert_eq!(
        pump_cap(&mut fixture, 4),
        ["KeyZ+", "KeyZ-", "KeyZ+", "KeyZ-"]
    );
    assert_eq!(
        pump_cap(&mut fixture, 4),
        ["KeyZ+", "KeyZ-", "KeyB+", "KeyB-"]
    );
    assert!(pump_cap(&mut fixture, 4).is_empty());

    // A focus loss recorded during the pump does not replace the detail handed over by it, and follows it.
    assert!(support::record_keys(1, code(Key::KeyA)));
    support::post_recording(Scheduled::FocusLost);
    assert_eq!(pump_cap(&mut fixture, 1000), ["KeyA+", "KeyA-"]);
    assert!(support::native_keys().focus_lost_pending);
    assert_eq!(pump_cap(&mut fixture, 1000), ["Lost"]);

    // An overflow caused during the pump is not folded into the pump's own hand-over.
    assert!(support::record_keys(1, code(Key::KeyA)));
    support::post_recording(Scheduled::KeyPairs(200, code(Key::KeyZ)));
    assert_eq!(pump_cap(&mut fixture, 1000), ["KeyA+", "KeyA-"]);
    assert!(support::native_keys().overflow);
    assert_eq!(pump_cap(&mut fixture, 1000), ["Resync"]);
    assert!(pump_cap(&mut fixture, 1000).is_empty());
}

/// A focus gain keeps its place among the keys recorded around it, whole and across caps, and with a resync.
pub(crate) fn a_focus_gain_keeps_its_place_among_the_keys_recorded_around_it() {
    let mut fixture = settled();
    let record = |key: Key| assert!(support::record_keys(1, code(key)));

    // Loss, key, gain, key: the gain sits between the keys, after the loss.
    support::resign_key();
    record(Key::KeyH);
    support::become_key();
    record(Key::KeyJ);
    assert_eq!(
        pump_cap(&mut fixture, 1000),
        ["Lost", "KeyH+", "KeyH-", "Gained", "KeyJ+", "KeyJ-"]
    );

    // The same, across pumps of two entries.
    support::resign_key();
    record(Key::KeyH);
    support::become_key();
    record(Key::KeyJ);
    assert_eq!(pump_cap(&mut fixture, 2), ["Lost", "KeyH+"]);
    assert_eq!(pump_cap(&mut fixture, 2), ["KeyH-", "Gained"]);
    assert_eq!(pump_cap(&mut fixture, 2), ["KeyJ+", "KeyJ-"]);
    assert!(pump_cap(&mut fixture, 2).is_empty());

    // A gain before every key, and after every key.
    support::become_key();
    record(Key::KeyH);
    assert_eq!(pump_cap(&mut fixture, 1000), ["Gained", "KeyH+", "KeyH-"]);
    record(Key::KeyH);
    support::become_key();
    assert_eq!(pump_cap(&mut fixture, 1000), ["KeyH+", "KeyH-", "Gained"]);

    // A gain with nothing around it is delivered alone, and one cap of 1 delivers one entry per pump.
    support::become_key();
    record(Key::KeyH);
    assert_eq!(pump_cap(&mut fixture, 1), ["Gained"]);
    assert_eq!(pump_cap(&mut fixture, 1), ["KeyH+"]);
    assert_eq!(pump_cap(&mut fixture, 1), ["KeyH-"]);
    assert!(pump_cap(&mut fixture, 1).is_empty());
}

/// With an overflow the resync stands where its first lost record stood: after a gain recorded before all lost
/// detail, before a gain recorded after some of it. The gain itself is never discarded.
pub(crate) fn a_focus_gain_and_a_resync_keep_the_order_of_their_first_records() {
    let mut fixture = settled();
    let record = |pairs: u32, key: Key| assert!(support::record_keys(pairs, code(key)));

    // The gain, then enough keys to overflow: nothing lost precedes it.
    support::become_key();
    record(150, Key::KeyZ);
    assert_eq!(pump_cap(&mut fixture, 1000), ["Gained", "Resync"]);
    assert!(pump_cap(&mut fixture, 1000).is_empty());
    support::become_key();
    record(150, Key::KeyZ);
    assert_eq!(pump_cap(&mut fixture, 1), ["Gained"]);
    assert_eq!(pump_cap(&mut fixture, 1), ["Resync"]);
    assert!(pump_cap(&mut fixture, 1).is_empty());

    // Keys, an overflow, then the gain: detail was lost before it.
    record(150, Key::KeyZ);
    support::become_key();
    assert_eq!(pump_cap(&mut fixture, 1000), ["Resync", "Gained"]);
    record(150, Key::KeyZ);
    support::become_key();
    assert_eq!(pump_cap(&mut fixture, 1), ["Resync"]);
    assert_eq!(pump_cap(&mut fixture, 1), ["Gained"]);
    assert!(pump_cap(&mut fixture, 1).is_empty());

    // Keys, the gain, then the overflow that discards the keys recorded before the gain.
    record(10, Key::KeyZ);
    support::become_key();
    record(150, Key::KeyZ);
    assert_eq!(pump_cap(&mut fixture, 1000), ["Resync", "Gained"]);
    assert!(pump_cap(&mut fixture, 1000).is_empty());

    // A loss still wins over everything pending, the gain included, and is not discarded.
    support::become_key();
    record(150, Key::KeyZ);
    support::resign_key();
    assert_eq!(pump_cap(&mut fixture, 1000), ["Lost"]);
    assert!(pump_cap(&mut fixture, 1000).is_empty());
}
