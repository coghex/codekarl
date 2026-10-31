//! Normalised keys and focus in the loop (CK-121): `Key`, `KeyResync`, `FocusLost` and `FocusGained` drive the loop's
//! `ck-input` mapper, and the close action replaces the hardcoded Escape on the native path.

use std::time::Duration;

use ck_input::{Bindings, Key, KeyStates};
use ck_window::{Answer, CloseSource, LoopEnd, LoopSettings, NativeEvent};

use crate::fakes::{DEADLINE, Rig, lock, owned, scenario};

fn press() -> ck_window::UiEvent {
    ck_window::UiEvent::CloseRequested {
        source: CloseSource::Key,
    }
}

fn down(key: Key) -> NativeEvent {
    NativeEvent::Key {
        key,
        down: true,
        repeat: false,
    }
}

fn up(key: Key) -> NativeEvent {
    NativeEvent::Key {
        key,
        down: false,
        repeat: false,
    }
}

fn repeat(key: Key) -> NativeEvent {
    NativeEvent::Key {
        key,
        down: true,
        repeat: true,
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

fn with(rig: &mut Rig, bindings: Bindings) {
    rig.settings = LoopSettings::default().with_bindings(bindings);
}

fn states(held: &[Key], pressed: &[Key], released: &[Key]) -> KeyStates {
    let mut states = KeyStates::new();
    for key in held {
        states.set_held(*key);
    }
    for key in pressed {
        states.set_pressed(*key);
    }
    for key in released {
        states.set_released(*key);
    }
    states
}

/// Runs a rig whose backend delivers `batches`, one per pump, answering every close with `answers`, until the loop
/// ends (an accepted close) or, when no answer accepts, the pump after the last batch (a quit). Returns the posted UI
/// events and how the loop ended. (An empty batch is only used while a close is outstanding, which keeps the loop
/// polling: the fake backend sleeps on a pump that delivers nothing when the loop asked to wait.)
fn run(
    batches: Vec<Vec<NativeEvent>>,
    answers: &[Answer],
    configure: impl FnOnce(&mut Rig),
) -> (Vec<ck_window::UiEvent>, LoopEnd) {
    let accepts = answers
        .iter()
        .any(|answer| matches!(answer, Answer::Accept | Answer::NotRegistered));
    let mut rig = Rig::new();
    configure(&mut rig);
    let batches_len = batches.len();
    for batch in batches {
        rig.backend.script.push_back(batch);
    }
    lock(&rig.ui.close_answers).extend(answers.iter().cloned());
    if !accepts {
        rig.quit_at_pump(batches_len + 1);
    }
    let ran = rig.run_collect();
    (ran.posts, ran.end)
}

#[test]
fn a_close_action_press_requests_a_close_with_the_source_key() {
    owned(
        module_path!(),
        "a_close_action_press_requests_a_close_with_the_source_key",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (posts, end) = run(vec![vec![down(Key::Escape)]], &[Answer::Accept], |_| {});
            assert_eq!(end, LoopEnd::Closed);
            assert_eq!(posts, [press()]);
        },
    );
}

#[test]
fn a_press_while_one_is_outstanding_is_absorbed_and_counted_in_the_existing_record() {
    owned(
        module_path!(),
        "a_press_while_one_is_outstanding_is_absorbed_and_counted_in_the_existing_record",
        DEADLINE,
        || {
            let scenario = scenario();
            // Down, up, down in one pass: two presses reach the loop, one request is made, the second is absorbed.
            let (posts, end) = run(
                vec![vec![down(Key::Escape), up(Key::Escape), down(Key::Escape)]],
                &[Answer::Decline],
                |_| {},
            );
            assert_eq!(end, LoopEnd::Quit);
            assert_eq!(posts, [press()], "one request");
            assert_eq!(
                scenario.records(),
                ["DEBUG window: 1 close request(s) absorbed while one was outstanding"]
            );
        },
    );
}

#[test]
fn a_held_escape_with_repeats_requests_one_close() {
    owned(
        module_path!(),
        "a_held_escape_with_repeats_requests_one_close",
        DEADLINE,
        || {
            let scenario = scenario();
            let mut batch = vec![down(Key::Escape)];
            batch.extend(std::iter::repeat_n(repeat(Key::Escape), 20));
            let (posts, end) = run(vec![batch], &[Answer::Decline], |_| {});
            assert_eq!(end, LoopEnd::Quit);
            assert_eq!(
                posts,
                [press()],
                "M4 counted each repeat as a press; a repeat is no press now"
            );
            assert!(
                scenario.records().is_empty(),
                "no absorbed press either: {:?}",
                scenario.records()
            );
        },
    );
}

#[test]
fn a_custom_close_binding_replaces_escape_on_the_native_path() {
    owned(
        module_path!(),
        "a_custom_close_binding_replaces_escape_on_the_native_path",
        DEADLINE,
        || {
            let _scenario = scenario();
            let custom = || bindings(&[("close", &["KeyQ"])]);
            // Escape is unbound: it requests nothing, down or up.
            let (posts, end) = run(vec![vec![down(Key::Escape), up(Key::Escape)]], &[], |rig| {
                with(rig, custom());
            });
            assert_eq!((posts.is_empty(), end), (true, LoopEnd::Quit), "{posts:?}");
            // Q requests a close.
            let (posts, end) = run(vec![vec![down(Key::KeyQ)]], &[Answer::Accept], |rig| {
                with(rig, custom());
            });
            assert_eq!((posts, end), (vec![press()], LoopEnd::Closed));
            // An explicit empty list unbinds the close action altogether.
            let (posts, end) = run(vec![vec![down(Key::Escape), down(Key::KeyQ)]], &[], |rig| {
                with(rig, bindings(&[("close", &[])]));
            });
            assert_eq!((posts.is_empty(), end), (true, LoopEnd::Quit));
        },
    );
}

#[test]
fn escape_from_the_glfw_path_still_requests_a_close_whatever_the_bindings() {
    owned(
        module_path!(),
        "escape_from_the_glfw_path_still_requests_a_close_whatever_the_bindings",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (posts, end) = run(
                vec![vec![NativeEvent::EscapePressed]],
                &[Answer::Accept],
                |rig| {
                    with(rig, bindings(&[("close", &["KeyQ"])]));
                },
            );
            assert_eq!((posts, end), (vec![press()], LoopEnd::Closed));
        },
    );
}

#[test]
fn focus_loss_releases_every_held_key_so_the_next_press_is_a_new_one() {
    owned(
        module_path!(),
        "focus_loss_releases_every_held_key_so_the_next_press_is_a_new_one",
        DEADLINE,
        || {
            let _scenario = scenario();
            // Escape goes down and its close is declined; the window loses focus (Escape is still down as far as the
            // loop knows); Escape goes down again. Without the reset the second down is a no-op, with it a new press.
            let batches = || {
                vec![
                    vec![down(Key::Escape)],
                    Vec::new(),
                    vec![NativeEvent::FocusLost, down(Key::Escape)],
                ]
            };
            let (posts, end) = run(batches(), &[Answer::Decline, Answer::Accept], |_| {});
            assert_eq!((posts, end), (vec![press(), press()], LoopEnd::Closed));
            // The control: no focus loss, and the second down is not a press.
            let without = vec![vec![down(Key::Escape)], Vec::new(), vec![down(Key::Escape)]];
            let (posts, end) = run(without, &[Answer::Decline], |_| {});
            assert_eq!((posts, end), (vec![press()], LoopEnd::Quit));
        },
    );
}

#[test]
fn gaining_focus_changes_no_input_state() {
    owned(
        module_path!(),
        "gaining_focus_changes_no_input_state",
        DEADLINE,
        || {
            let _scenario = scenario();
            // Escape held and declined, focus gained, a repeat: still held, no new press.
            let (posts, end) = run(
                vec![
                    vec![down(Key::Escape)],
                    Vec::new(),
                    vec![NativeEvent::FocusGained, repeat(Key::Escape)],
                ],
                &[Answer::Decline],
                |_| {},
            );
            assert_eq!((posts, end), (vec![press()], LoopEnd::Quit));
        },
    );
}

#[test]
fn a_resync_applies_the_final_states_and_a_lost_escape_tap_still_closes() {
    owned(
        module_path!(),
        "a_resync_applies_the_final_states_and_a_lost_escape_tap_still_closes",
        DEADLINE,
        || {
            let _scenario = scenario();
            // A complete Escape tap in the discarded span: up now, press and release latched.
            let tap = states(&[], &[Key::Escape], &[Key::Escape]);
            let (posts, end) = run(
                vec![vec![NativeEvent::KeyResync(tap)]],
                &[Answer::Accept],
                |_| {},
            );
            assert_eq!((posts, end), (vec![press()], LoopEnd::Closed));
            // A resync that only moves a non-close key requests nothing.
            let (posts, end) = run(
                vec![vec![NativeEvent::KeyResync(states(
                    &[Key::KeyR],
                    &[Key::KeyR],
                    &[],
                ))]],
                &[],
                |_| {},
            );
            assert_eq!((posts.is_empty(), end), (true, LoopEnd::Quit));
            // A prefixed close binding is not closed by lost taps (schedule A), but is by a chord held now.
            let chord = || bindings(&[("close", &["Control+KeyR"])]);
            let lost = states(&[], &[Key::KeyR], &[Key::KeyR]);
            let (posts, end) = run(vec![vec![NativeEvent::KeyResync(lost)]], &[], |rig| {
                with(rig, chord());
            });
            assert_eq!((posts.is_empty(), end), (true, LoopEnd::Quit), "{posts:?}");
            let held = states(&[Key::ControlLeft, Key::KeyR], &[Key::KeyR], &[]);
            let (posts, end) = run(
                vec![vec![NativeEvent::KeyResync(held)]],
                &[Answer::Accept],
                |rig| {
                    with(rig, chord());
                },
            );
            assert_eq!((posts, end), (vec![press()], LoopEnd::Closed));
        },
    );
}

#[test]
fn the_loop_settings_carry_the_default_bindings_until_told_otherwise() {
    let settings = LoopSettings::new(64, 16, Duration::from_secs(2), Duration::from_millis(100))
        .expect("valid settings");
    assert_eq!(settings.bindings(), LoopSettings::default().bindings());
    assert_eq!(settings.bindings(), &bindings(&[]));
    let custom = bindings(&[("close", &["KeyQ"])]);
    let changed = settings.clone().with_bindings(custom.clone());
    assert_eq!(changed.bindings(), &custom);
    assert_eq!(changed.native_events(), 64, "nothing else changed");
}
