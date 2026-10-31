//! The bounded callback state GLFW's pump hands over.
//!
//! Callbacks only update fixed-size state and never call GLFW. The callback and pump both run on the
//! process's main thread, but `RefCell` keeps the callback boundary explicit without making the state
//! `Sync` or introducing locks.

use std::cell::{Cell, RefCell};

use ck_input::{Key, KeyStates, TRANSITION_CAPACITY};
use ck_window::{NativeEvent, Wait};

/// GLFW names these tokens for US-layout positions on every supported platform (glfw3.h, Keyboard keys).
const KEY_MAP: [(i32, Key); ck_input::KEY_COUNT] = [
    (crate::ffi::KEY_A, Key::KeyA),
    (crate::ffi::KEY_B, Key::KeyB),
    (crate::ffi::KEY_C, Key::KeyC),
    (crate::ffi::KEY_D, Key::KeyD),
    (crate::ffi::KEY_E, Key::KeyE),
    (crate::ffi::KEY_F, Key::KeyF),
    (crate::ffi::KEY_G, Key::KeyG),
    (crate::ffi::KEY_H, Key::KeyH),
    (crate::ffi::KEY_I, Key::KeyI),
    (crate::ffi::KEY_J, Key::KeyJ),
    (crate::ffi::KEY_K, Key::KeyK),
    (crate::ffi::KEY_L, Key::KeyL),
    (crate::ffi::KEY_M, Key::KeyM),
    (crate::ffi::KEY_N, Key::KeyN),
    (crate::ffi::KEY_O, Key::KeyO),
    (crate::ffi::KEY_P, Key::KeyP),
    (crate::ffi::KEY_Q, Key::KeyQ),
    (crate::ffi::KEY_R, Key::KeyR),
    (crate::ffi::KEY_S, Key::KeyS),
    (crate::ffi::KEY_T, Key::KeyT),
    (crate::ffi::KEY_U, Key::KeyU),
    (crate::ffi::KEY_V, Key::KeyV),
    (crate::ffi::KEY_W, Key::KeyW),
    (crate::ffi::KEY_X, Key::KeyX),
    (crate::ffi::KEY_Y, Key::KeyY),
    (crate::ffi::KEY_Z, Key::KeyZ),
    (crate::ffi::KEY_0, Key::Digit0),
    (crate::ffi::KEY_1, Key::Digit1),
    (crate::ffi::KEY_2, Key::Digit2),
    (crate::ffi::KEY_3, Key::Digit3),
    (crate::ffi::KEY_4, Key::Digit4),
    (crate::ffi::KEY_5, Key::Digit5),
    (crate::ffi::KEY_6, Key::Digit6),
    (crate::ffi::KEY_7, Key::Digit7),
    (crate::ffi::KEY_8, Key::Digit8),
    (crate::ffi::KEY_9, Key::Digit9),
    (crate::ffi::KEY_F1, Key::F1),
    (crate::ffi::KEY_F2, Key::F2),
    (crate::ffi::KEY_F3, Key::F3),
    (crate::ffi::KEY_F4, Key::F4),
    (crate::ffi::KEY_F5, Key::F5),
    (crate::ffi::KEY_F6, Key::F6),
    (crate::ffi::KEY_F7, Key::F7),
    (crate::ffi::KEY_F8, Key::F8),
    (crate::ffi::KEY_F9, Key::F9),
    (crate::ffi::KEY_F10, Key::F10),
    (crate::ffi::KEY_F11, Key::F11),
    (crate::ffi::KEY_F12, Key::F12),
    (crate::ffi::KEY_UP, Key::ArrowUp),
    (crate::ffi::KEY_DOWN, Key::ArrowDown),
    (crate::ffi::KEY_LEFT, Key::ArrowLeft),
    (crate::ffi::KEY_RIGHT, Key::ArrowRight),
    (crate::ffi::KEY_ESCAPE, Key::Escape),
    (crate::ffi::KEY_SPACE, Key::Space),
    (crate::ffi::KEY_ENTER, Key::Enter),
    (crate::ffi::KEY_TAB, Key::Tab),
    (crate::ffi::KEY_BACKSPACE, Key::Backspace),
    (crate::ffi::KEY_DELETE, Key::Delete),
    (crate::ffi::KEY_HOME, Key::Home),
    (crate::ffi::KEY_END, Key::End),
    (crate::ffi::KEY_PAGE_UP, Key::PageUp),
    (crate::ffi::KEY_PAGE_DOWN, Key::PageDown),
    (crate::ffi::KEY_MINUS, Key::Minus),
    (crate::ffi::KEY_EQUAL, Key::Equal),
    (crate::ffi::KEY_LEFT_BRACKET, Key::BracketLeft),
    (crate::ffi::KEY_RIGHT_BRACKET, Key::BracketRight),
    (crate::ffi::KEY_BACKSLASH, Key::Backslash),
    (crate::ffi::KEY_SEMICOLON, Key::Semicolon),
    (crate::ffi::KEY_APOSTROPHE, Key::Quote),
    (crate::ffi::KEY_COMMA, Key::Comma),
    (crate::ffi::KEY_PERIOD, Key::Period),
    (crate::ffi::KEY_SLASH, Key::Slash),
    (crate::ffi::KEY_GRAVE_ACCENT, Key::Backquote),
    (crate::ffi::KEY_LEFT_SHIFT, Key::ShiftLeft),
    (crate::ffi::KEY_RIGHT_SHIFT, Key::ShiftRight),
    (crate::ffi::KEY_LEFT_CONTROL, Key::ControlLeft),
    (crate::ffi::KEY_RIGHT_CONTROL, Key::ControlRight),
    (crate::ffi::KEY_LEFT_ALT, Key::AltLeft),
    (crate::ffi::KEY_RIGHT_ALT, Key::AltRight),
    (crate::ffi::KEY_LEFT_SUPER, Key::MetaLeft),
    (crate::ffi::KEY_RIGHT_SUPER, Key::MetaRight),
];

pub(crate) fn key_from_glfw(token: i32) -> Option<Key> {
    if token == crate::ffi::KEY_UNKNOWN {
        return None;
    }
    KEY_MAP
        .iter()
        .find_map(|(candidate, key)| (*candidate == token).then_some(*key))
}

/// What caused the close request that is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseSource {
    /// The window's close button (GLFW's close callback).
    Button,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transition {
    key: Key,
    down: bool,
    repeat: bool,
}

/// Key/focus state kept separate from the small window-state cells.
#[derive(Debug)]
struct InputPending {
    held: [bool; ck_input::KEY_COUNT],
    ring: [Option<Transition>; TRANSITION_CAPACITY],
    head: usize,
    len: usize,
    latches: KeyStates,
    overflow: bool,
    focus_lost: bool,
    focus_gain_at: Option<usize>,
    focus_gain_after_lost: bool,
}

impl Default for InputPending {
    fn default() -> Self {
        Self {
            held: [false; ck_input::KEY_COUNT],
            ring: [None; TRANSITION_CAPACITY],
            head: 0,
            len: 0,
            latches: KeyStates::new(),
            overflow: false,
            focus_lost: false,
            focus_gain_at: None,
            focus_gain_after_lost: false,
        }
    }
}

impl InputPending {
    fn record_key(&mut self, key: Key, down: bool, repeat: bool) {
        let index = key as usize;
        let was_down = self.held[index];
        if repeat {
            if !down || !was_down {
                return;
            }
        } else if was_down == down {
            return;
        }
        self.held[index] = down;
        if self.overflow {
            if !repeat {
                latch(&mut self.latches, key, down);
            }
            return;
        }
        if self.len == TRANSITION_CAPACITY {
            let discarded = self.ring[self.head]
                .take()
                .expect("a full ring has a transition at its head");
            if !discarded.repeat {
                latch(&mut self.latches, discarded.key, discarded.down);
            }
            self.head = (self.head + 1) % TRANSITION_CAPACITY;
            self.len -= 1;
            self.overflow = true;
            if let Some(position) = &mut self.focus_gain_at
                && *position > 0
            {
                *position -= 1;
                self.focus_gain_after_lost = true;
            }
        }
        let tail = (self.head + self.len) % TRANSITION_CAPACITY;
        self.ring[tail] = Some(Transition { key, down, repeat });
        self.len += 1;
    }

    fn focus_lost(&mut self) {
        self.held.fill(false);
        self.ring = [None; TRANSITION_CAPACITY];
        self.head = 0;
        self.len = 0;
        self.latches = KeyStates::new();
        self.overflow = false;
        self.focus_lost = true;
        self.focus_gain_at = None;
        self.focus_gain_after_lost = false;
    }

    fn focus_gained(&mut self) {
        if self.focus_gain_at.is_none() {
            self.focus_gain_at = Some(self.len);
            // If overflow has already discarded pre-gain detail, the gain follows that lost prefix.
            self.focus_gain_after_lost = self.overflow;
        }
    }

    fn is_pending(&self) -> bool {
        self.len > 0 || self.overflow || self.focus_lost || self.focus_gain_at.is_some()
    }

    fn take(&mut self) -> Vec<NativeEvent> {
        let mut out = Vec::with_capacity(self.len + 3);
        if std::mem::take(&mut self.focus_lost) {
            out.push(NativeEvent::FocusLost);
        }

        if self.overflow {
            while self.len > 0 {
                let transition = self.ring[self.head]
                    .take()
                    .expect("a queued position has a transition");
                if !transition.repeat {
                    latch(&mut self.latches, transition.key, transition.down);
                }
                self.head = (self.head + 1) % TRANSITION_CAPACITY;
                self.len -= 1;
            }
            let states = self.states();
            let gain_before_resync = self.focus_gain_at == Some(0) && !self.focus_gain_after_lost;
            if gain_before_resync {
                out.push(NativeEvent::FocusGained);
            }
            out.push(NativeEvent::KeyResync(states));
            if self.focus_gain_at.is_some() && !gain_before_resync {
                out.push(NativeEvent::FocusGained);
            }
        } else {
            let gain_at = self.focus_gain_at;
            for position in 0..=self.len {
                if gain_at == Some(position) {
                    out.push(NativeEvent::FocusGained);
                }
                if position < self.len {
                    let transition = self.ring[self.head]
                        .take()
                        .expect("a queued position has a transition");
                    out.push(NativeEvent::Key {
                        key: transition.key,
                        down: transition.down,
                        repeat: transition.repeat,
                    });
                    self.head = (self.head + 1) % TRANSITION_CAPACITY;
                }
            }
            self.len = 0;
            self.head = 0;
        }

        self.latches = KeyStates::new();
        self.overflow = false;
        self.focus_gain_at = None;
        self.focus_gain_after_lost = false;
        out
    }

    fn states(&self) -> KeyStates {
        let mut states = self.latches;
        for key in Key::ALL {
            if self.held[key as usize] {
                states.set_held(key);
            }
        }
        states
    }
}

fn latch(states: &mut KeyStates, key: Key, down: bool) {
    if down {
        states.set_pressed(key);
    } else {
        states.set_released(key);
    }
}

/// The pending state: close, latest pixel size, size query, redraw, focus/minimise, and bounded key input.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    close: Cell<Option<CloseSource>>,
    absorbed: Cell<u32>,
    size: Cell<Option<(u32, u32)>>,
    query_size: Cell<bool>,
    display: Cell<bool>,
    minimized: Cell<Option<bool>>,
    input: RefCell<InputPending>,
}

/// What one pump takes out of the pending state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Taken {
    pub(crate) close: Option<CloseSource>,
    pub(crate) absorbed: u32,
    pub(crate) size: Option<(u32, u32)>,
    pub(crate) query_size: bool,
    pub(crate) display: bool,
    pub(crate) minimized: Option<bool>,
    pub(crate) input: Vec<NativeEvent>,
}

impl Pending {
    /// Records a close-button request. A request already waiting keeps its source and absorbs later presses.
    pub(crate) fn record_close(&self) {
        if self.close.get().is_none() {
            self.close.set(Some(CloseSource::Button));
        } else {
            self.absorbed.set(self.absorbed.get().saturating_add(1));
        }
    }

    /// Records a key transition if it changes held state, or a repeat for a held key.
    pub(crate) fn record_key(&self, key: Key, down: bool, repeat: bool) {
        self.input.borrow_mut().record_key(key, down, repeat);
    }

    /// Records focus loss and resets all older key detail; later callbacks remain ordered after it.
    pub(crate) fn record_focus_lost(&self) {
        self.input.borrow_mut().focus_lost();
    }

    /// Records one pending focus gain at its position among retained key records.
    pub(crate) fn record_focus_gained(&self) {
        self.input.borrow_mut().focus_gained();
    }

    /// Records the latest iconify state for this drain.
    pub(crate) fn record_minimized(&self, minimized: bool) {
        self.minimized.set(Some(minimized));
    }

    /// Records the latest framebuffer size (the callback's own values).
    pub(crate) fn record_size(&self, width: u32, height: u32) {
        self.size.set(Some((width, height)));
    }

    /// Records that the content scale changed: the pump reads the size once GLFW has returned.
    pub(crate) fn record_scale_changed(&self) {
        self.query_size.set(true);
    }

    /// Records that the window needs to be drawn again.
    pub(crate) fn record_display(&self) {
        self.display.set(true);
    }

    /// Whether anything is waiting to be handed over.
    pub(crate) fn is_pending(&self) -> bool {
        self.close.get().is_some()
            || self.size.get().is_some()
            || self.query_size.get()
            || self.display.get()
            || self.minimized.get().is_some()
            || self.input.borrow().is_pending()
    }

    /// Takes everything and clears the state.
    pub(crate) fn take(&self) -> Taken {
        Taken {
            close: self.close.take(),
            absorbed: self.absorbed.take(),
            size: self.size.take(),
            query_size: self.query_size.take(),
            display: self.display.take(),
            minimized: self.minimized.take(),
            input: self.input.borrow_mut().take(),
        }
    }
}

/// The wait a pump really makes: a blocking pump with state already waiting does not block.
pub(crate) fn effective_wait(wait: Wait, pending: bool) -> Wait {
    if pending { Wait::Poll } else { wait }
}

/// Convert a deadline to GLFW's required positive finite seconds, clamping very long waits.
pub(crate) fn timeout_seconds(
    deadline: std::time::Instant,
    now: std::time::Instant,
) -> Option<f64> {
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        None
    } else {
        Some(
            remaining
                .min(std::time::Duration::from_secs(86_400))
                .as_secs_f64(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ck_input::Key;

    use super::*;

    #[test]
    fn every_supported_input_key_has_one_glfw_token_and_unknowns_are_dropped() {
        assert_eq!(KEY_MAP.len(), Key::ALL.len());
        for (index, (token, key)) in KEY_MAP.iter().enumerate() {
            assert_eq!(key_from_glfw(*token), Some(*key));
            assert!(
                KEY_MAP[..index]
                    .iter()
                    .all(|(prior, prior_key)| { prior != token && prior_key != key })
            );
        }
        for token in [crate::ffi::KEY_UNKNOWN, 0, 400] {
            assert_eq!(key_from_glfw(token), None);
        }
    }

    #[test]
    fn records_only_key_state_changes_and_repeats_while_held() {
        let pending = Pending::default();
        pending.record_key(Key::KeyA, false, false);
        pending.record_key(Key::KeyA, true, false);
        pending.record_key(Key::KeyA, true, false);
        pending.record_key(Key::KeyA, true, true);
        pending.record_key(Key::KeyA, false, true);
        pending.record_key(Key::KeyA, false, false);
        assert_eq!(
            pending.take().input,
            [
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: true,
                    repeat: false
                },
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: true,
                    repeat: true
                },
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: false,
                    repeat: false
                },
            ]
        );
    }

    #[test]
    fn overflow_hands_over_one_resync_with_final_state_and_discarded_latches() {
        let pending = Pending::default();
        for _ in 0..150 {
            pending.record_key(Key::KeyA, true, false);
            pending.record_key(Key::KeyA, false, false);
        }
        let taken = pending.take();
        assert_eq!(taken.input.len(), 1);
        let [NativeEvent::KeyResync(states)] = taken.input.as_slice() else {
            panic!("overflow is a single resync: {:?}", taken.input);
        };
        assert!(!states.is_held(Key::KeyA));
        assert!(states.was_pressed(Key::KeyA));
        assert!(states.was_released(Key::KeyA));
        assert!(!pending.is_pending());
        assert!(pending.take().input.is_empty());
    }

    #[test]
    fn repeats_discarded_by_overflow_do_not_become_pressed_latches() {
        let pending = Pending::default();
        pending.record_key(Key::KeyA, true, false);
        assert_eq!(
            pending.take().input,
            [NativeEvent::Key {
                key: Key::KeyA,
                down: true,
                repeat: false
            }]
        );

        for _ in 0..=TRANSITION_CAPACITY {
            pending.record_key(Key::KeyA, true, true);
        }
        let taken = pending.take();
        let [NativeEvent::KeyResync(states)] = taken.input.as_slice() else {
            panic!("discarded repeats produce one resync: {:?}", taken.input);
        };
        assert!(states.is_held(Key::KeyA));
        assert!(!states.was_pressed(Key::KeyA));
        assert!(!states.was_released(Key::KeyA));
        assert!(!pending.is_pending());
        assert!(pending.take().input.is_empty());
    }

    #[test]
    fn escape_tap_entirely_in_discarded_span_is_latched_once() {
        let pending = Pending::default();
        for _ in 0..150 {
            pending.record_key(Key::Escape, true, false);
            pending.record_key(Key::Escape, false, false);
        }
        let taken = pending.take();
        let [NativeEvent::KeyResync(states)] = taken.input.as_slice() else {
            panic!("the discarded span is one resync: {:?}", taken.input);
        };
        assert!(!states.is_held(Key::Escape));
        assert!(states.was_pressed(Key::Escape));
        assert!(states.was_released(Key::Escape));
        assert!(pending.take().input.is_empty());
    }

    #[test]
    fn focus_loss_cancels_older_detail_and_gain_but_keeps_later_keys_after_loss() {
        let pending = Pending::default();
        pending.record_key(Key::KeyA, true, false);
        pending.record_focus_gained();
        pending.record_focus_lost();
        pending.record_key(Key::KeyB, true, false);
        assert_eq!(
            pending.take().input,
            [
                NativeEvent::FocusLost,
                NativeEvent::Key {
                    key: Key::KeyB,
                    down: true,
                    repeat: false
                },
            ]
        );
    }

    fn fold_focus(initial: bool, events: &[NativeEvent]) -> bool {
        events.iter().fold(initial, |focused, event| match event {
            NativeEvent::FocusLost => false,
            NativeEvent::FocusGained => true,
            _ => focused,
        })
    }

    #[test]
    fn focus_sequences_fold_to_the_final_window_focus_and_do_not_replay() {
        let gain_then_loss = Pending::default();
        gain_then_loss.record_focus_gained();
        gain_then_loss.record_focus_lost();
        let events = gain_then_loss.take().input;
        assert_eq!(events, [NativeEvent::FocusLost]);
        assert!(!fold_focus(true, &events));
        assert!(gain_then_loss.take().input.is_empty());

        let loss_then_gain = Pending::default();
        loss_then_gain.record_focus_lost();
        loss_then_gain.record_focus_gained();
        let events = loss_then_gain.take().input;
        assert_eq!(events, [NativeEvent::FocusLost, NativeEvent::FocusGained]);
        assert!(fold_focus(true, &events));
        assert!(loss_then_gain.take().input.is_empty());

        let loss_gain_loss = Pending::default();
        loss_gain_loss.record_focus_lost();
        loss_gain_loss.record_focus_gained();
        loss_gain_loss.record_focus_lost();
        let events = loss_gain_loss.take().input;
        assert_eq!(events, [NativeEvent::FocusLost]);
        assert!(!fold_focus(true, &events));
        assert!(loss_gain_loss.take().input.is_empty());
    }

    #[test]
    fn focus_loss_discards_pending_overflow_and_glfw_synthetic_release() {
        let pending = Pending::default();
        for _ in 0..150 {
            pending.record_key(Key::KeyA, true, false);
            pending.record_key(Key::KeyA, false, false);
        }
        pending.record_focus_gained();
        pending.record_focus_lost();
        pending.record_key(Key::KeyA, false, false);
        let taken = pending.take();
        assert_eq!(taken.input, [NativeEvent::FocusLost]);
        let states = pending.input.borrow().states();
        assert!(!states.is_held(Key::KeyA));
        assert!(!states.was_pressed(Key::KeyA));
        assert!(!states.was_released(Key::KeyA));
        assert!(pending.take().input.is_empty());
    }

    #[test]
    fn focus_gain_and_loss_are_ordered_around_keys_and_loss_cancels_its_gain() {
        let pending = Pending::default();
        pending.record_key(Key::KeyA, true, false);
        pending.record_focus_gained();
        pending.record_key(Key::KeyB, true, false);
        assert_eq!(
            pending.take().input,
            [
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: true,
                    repeat: false
                },
                NativeEvent::FocusGained,
                NativeEvent::Key {
                    key: Key::KeyB,
                    down: true,
                    repeat: false
                },
            ]
        );
        assert!(pending.take().input.is_empty());
    }

    #[test]
    fn a_delivered_tap_is_not_latched_again_by_a_later_overflow() {
        let pending = Pending::default();
        pending.record_key(Key::KeyA, true, false);
        pending.record_key(Key::KeyA, false, false);
        assert_eq!(
            pending.take().input,
            [
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: true,
                    repeat: false
                },
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: false,
                    repeat: false
                },
            ]
        );

        for _ in 0..130 {
            pending.record_key(Key::KeyB, true, false);
            pending.record_key(Key::KeyB, false, false);
        }
        let taken = pending.take();
        let [NativeEvent::KeyResync(states)] = taken.input.as_slice() else {
            panic!("the later discarded span is one resync");
        };
        assert!(!states.was_pressed(Key::KeyA));
        assert!(!states.was_released(Key::KeyA));
        assert!(pending.take().input.is_empty());
    }

    #[test]
    fn focus_gain_keeps_its_position_and_overflow_resync_order() {
        let between = Pending::default();
        between.record_key(Key::KeyA, true, false);
        between.record_focus_gained();
        between.record_key(Key::KeyB, true, false);
        let events = between.take().input;
        assert_eq!(
            events,
            [
                NativeEvent::Key {
                    key: Key::KeyA,
                    down: true,
                    repeat: false
                },
                NativeEvent::FocusGained,
                NativeEvent::Key {
                    key: Key::KeyB,
                    down: true,
                    repeat: false
                },
            ]
        );
        assert!(fold_focus(true, &events));
        assert!(between.take().input.is_empty());

        let before_lost = Pending::default();
        before_lost.record_focus_gained();
        for _ in 0..150 {
            before_lost.record_key(Key::KeyA, true, false);
            before_lost.record_key(Key::KeyA, false, false);
        }
        let first = before_lost.take().input;
        assert!(matches!(
            first.as_slice(),
            [NativeEvent::FocusGained, NativeEvent::KeyResync(_)]
        ));
        assert!(fold_focus(true, &first));
        assert!(before_lost.take().input.is_empty());

        let after_lost = Pending::default();
        for _ in 0..150 {
            after_lost.record_key(Key::KeyA, true, false);
            after_lost.record_key(Key::KeyA, false, false);
        }
        after_lost.record_focus_gained();
        let second = after_lost.take().input;
        assert!(matches!(
            second.as_slice(),
            [NativeEvent::KeyResync(_), NativeEvent::FocusGained]
        ));
        assert!(fold_focus(true, &second));
        assert!(after_lost.take().input.is_empty());
    }

    #[test]
    fn pending_window_state_coalesces_and_close_absorption_saturates() {
        let pending = Pending::default();
        pending.record_close();
        pending.record_close();
        pending.record_close();
        pending.record_size(320, 200);
        pending.record_size(800, 500);
        pending.record_scale_changed();
        pending.record_scale_changed();
        pending.record_minimized(true);
        pending.record_minimized(false);
        pending.record_display();
        let taken = pending.take();
        assert_eq!(taken.close, Some(CloseSource::Button));
        assert_eq!(taken.absorbed, 2);
        assert_eq!(taken.size, Some((800, 500)));
        assert!(taken.query_size);
        assert_eq!(taken.minimized, Some(false));
        assert!(taken.display);
        assert!(!pending.is_pending());

        let saturating = Pending::default();
        saturating.record_close();
        saturating.absorbed.set(u32::MAX - 1);
        saturating.record_close();
        saturating.record_close();
        assert_eq!(saturating.take().absorbed, u32::MAX);
    }

    #[test]
    fn any_pending_input_or_window_state_turns_a_blocking_wait_into_a_poll() {
        let empty = Pending::default();
        assert_eq!(
            effective_wait(Wait::UntilEvent, empty.is_pending()),
            Wait::UntilEvent
        );

        let mut pending_states = Vec::new();
        let close = Pending::default();
        close.record_close();
        pending_states.push(close);
        let size = Pending::default();
        size.record_size(1, 2);
        pending_states.push(size);
        let scale = Pending::default();
        scale.record_scale_changed();
        pending_states.push(scale);
        let display = Pending::default();
        display.record_display();
        pending_states.push(display);
        let minimized = Pending::default();
        minimized.record_minimized(true);
        pending_states.push(minimized);
        let key = Pending::default();
        key.record_key(Key::KeyA, true, false);
        pending_states.push(key);
        let lost = Pending::default();
        lost.record_focus_lost();
        pending_states.push(lost);
        let gained = Pending::default();
        gained.record_focus_gained();
        pending_states.push(gained);

        for pending in pending_states {
            assert!(pending.is_pending());
            assert_eq!(
                effective_wait(Wait::UntilEvent, pending.is_pending()),
                Wait::Poll
            );
        }
    }

    #[test]
    fn timed_wait_uses_positive_finite_remaining_time_and_clamps_to_one_day() {
        let now = Instant::now();
        assert_eq!(timeout_seconds(now, now), None);
        assert_eq!(
            timeout_seconds(now.checked_sub(Duration::from_nanos(1)).unwrap(), now),
            None
        );
        assert_eq!(
            timeout_seconds(now + Duration::from_nanos(1), now),
            Some(1e-9)
        );
        assert_eq!(
            timeout_seconds(now + Duration::from_secs(172_800), now),
            Some(86_400.0)
        );
        assert_eq!(
            timeout_seconds(now + Duration::from_secs(86_400), now),
            Some(86_400.0)
        );
    }
}
