//! Typed text on Cocoa (CK-314): the content view as the system's text input client, through the real backend: a posted key-down
//! carrying characters, the five edit commands, a repeat, composition committed as one precomposed character, Escape, and text input off.
//! All use **marked** synthetic events through the real dispatch, with the owner's input ignored as in the rest of this group, and leave
//! the system's keyboard layout alone: the key codes are the ANSI positions, and the characters the events carry are what the layout the
//! machine has would give (the cases use the plain ASCII ones).

use ck_input::{Key, TypedEntry};
use ck_window::{Backend, NativeEvent};

use crate::Fixture;
use crate::support::{self, Pumped};

const SHIFT: u64 = 1 << 17;
const COMMAND: u64 = 1 << 20;

fn code(key: Key) -> u16 {
    support::key_table()
        .iter()
        .find(|(_, _, name)| name == key.name())
        .map_or_else(
            || panic!("{} is not in the key table", key.name()),
            |(code, _, _)| *code,
        )
}

/// Posts a marked key-down of `key` with `characters` and `flags`.
fn type_key(key: Key, flags: u64, characters: &str) {
    assert!(
        support::post_key(code(key), flags, false, characters),
        "{key:?}"
    );
}

/// The typed entries a pump handed over, in order.
fn typed(pumped: &Pumped) -> Vec<TypedEntry> {
    pumped
        .input
        .iter()
        .filter_map(|event| match event {
            NativeEvent::Typed(entry) => Some(*entry),
            _ => None,
        })
        .collect()
}

fn open_with_text_input() -> Fixture {
    let mut fixture = Fixture::open(300, 200);
    fixture.backend.set_text_input(true);
    fixture
}

/// A posted key-down carrying `a` gives one character entry `a`, one carrying `A` gives `A`, and the keys are recorded as keys too.
pub(crate) fn a_key_down_with_characters_gives_one_character_entry() {
    let mut fixture = open_with_text_input();
    type_key(Key::KeyA, 0, "a");
    let pumped = fixture.drain();
    assert_eq!(typed(&pumped), [TypedEntry::Char('a')]);
    assert_eq!(
        pumped.keys(),
        [(Key::KeyA, true, false)],
        "also a physical key"
    );
    type_key(Key::KeyA, SHIFT, "A");
    assert_eq!(typed(&fixture.drain()), [TypedEntry::Char('A')]);
}

/// Backspace, Enter, Tab, Left and Right give their five commands, and several keys keep their order.
pub(crate) fn the_five_edit_commands_and_several_keys_keep_their_order() {
    let mut fixture = open_with_text_input();
    type_key(Key::KeyA, 0, "a");
    type_key(Key::Backspace, 0, "\u{7f}");
    type_key(Key::Enter, 0, "\r");
    type_key(Key::Tab, 0, "\t");
    type_key(Key::ArrowLeft, 0, "\u{f702}");
    type_key(Key::ArrowRight, 0, "\u{f703}");
    type_key(Key::KeyB, 0, "b");
    assert_eq!(
        typed(&fixture.drain()),
        [
            TypedEntry::Char('a'),
            TypedEntry::Backspace,
            TypedEntry::Enter,
            TypedEntry::Tab,
            TypedEntry::Left,
            TypedEntry::Right,
            TypedEntry::Char('b'),
        ]
    );
}

/// A key-down marked as a repeat gives another entry.
pub(crate) fn a_repeated_key_down_gives_another_entry() {
    let mut fixture = open_with_text_input();
    type_key(Key::KeyA, 0, "a");
    assert!(support::post_key(code(Key::KeyA), 0, true, "a"));
    assert_eq!(
        typed(&fixture.drain()),
        [TypedEntry::Char('a'), TypedEntry::Char('a')]
    );
}

/// Text set as being composed and then committed as `á` gives one entry, `á`, and nothing for the composing text; a commit of `a`
/// followed by U+0301 in one string gives the one entry `á` (precomposed).
pub(crate) fn composed_text_gives_one_precomposed_entry_and_nothing_while_composing() {
    let mut fixture = open_with_text_input();
    support::set_marked_text("\u{b4}");
    assert!(support::has_marked_text());
    assert!(
        typed(&fixture.drain()).is_empty(),
        "text being composed is not an entry"
    );
    support::insert_text("\u{e1}");
    assert!(!support::has_marked_text(), "committing ends the composing");
    assert_eq!(typed(&fixture.drain()), [TypedEntry::Char('\u{e1}')]);
    support::insert_text("a\u{301}");
    assert_eq!(
        typed(&fixture.drain()),
        [TypedEntry::Char('\u{e1}')],
        "a and U+0301 are made precomposed: one entry"
    );
    // A control character and the system's private function-key codes are never entries.
    support::insert_text("\u{1}\u{f700}x\u{7f}\u{85}");
    assert_eq!(typed(&fixture.drain()), [TypedEntry::Char('x')]);
}

/// Escape gives no entry, and its key is recorded: it stays the `close` action. A key-down with Command held is not given to text
/// input either.
pub(crate) fn escape_and_a_command_chord_give_no_entry_and_the_keys_are_recorded() {
    let mut fixture = open_with_text_input();
    type_key(Key::Escape, 0, "\u{1b}");
    let pumped = fixture.drain();
    assert!(typed(&pumped).is_empty(), "{:?}", typed(&pumped));
    assert_eq!(pumped.keys(), [(Key::Escape, true, false)]);
    type_key(Key::KeyA, COMMAND, "a");
    let pumped = fixture.drain();
    assert!(typed(&pumped).is_empty(), "{:?}", typed(&pumped));
    assert!(pumped.keys().contains(&(Key::KeyA, true, false)));
}

/// With text input off (the start), the same key-downs give no entry and are recorded as keys; turning it on and off discards text
/// being composed and the entries not yet handed over.
pub(crate) fn with_text_input_off_there_are_no_entries_and_turning_it_off_discards_what_waits() {
    let mut fixture = Fixture::open(300, 200);
    type_key(Key::KeyA, 0, "a");
    type_key(Key::Backspace, 0, "\u{7f}");
    let pumped = fixture.drain();
    assert!(typed(&pumped).is_empty(), "off at the start");
    assert_eq!(
        pumped.keys(),
        [(Key::KeyA, true, false), (Key::Backspace, true, false)]
    );
    // Recorded but not handed over, then turned off: what waits is dropped.
    fixture.backend.set_text_input(true);
    support::set_marked_text("\u{b4}");
    type_key(Key::KeyA, 0, "a");
    support::drain_events();
    fixture.backend.set_text_input(false);
    assert!(!support::has_marked_text(), "composing text is discarded");
    assert!(
        typed(&fixture.drain()).is_empty(),
        "entries not yet delivered are dropped"
    );
    // And turning it on starts with none.
    type_key(Key::KeyA, 0, "a");
    support::drain_events();
    fixture.backend.set_text_input(true);
    assert!(typed(&fixture.drain()).is_empty(), "on starts empty");
}
