//! Portable physical-key names, validated action bindings, and a bounded pure input mapper.
//!
//! This crate owns no native keys, window APIs, settings files, logging, dependencies, or unsafe code.
//! A [`Mapper`] is a single-owner state machine; its key-event methods use fixed storage and allocate
//! nothing. Snapshots copy a coherent view and only [`Mapper::acknowledge`] consumes reported edges.

#![deny(unsafe_code)]

use std::{error::Error, fmt};

/// Maximum number of supported physical keys in this milestone.
pub const KEY_COUNT: usize = 81;

/// Capacity of the ordered action-transition ring.
pub const TRANSITION_CAPACITY: usize = 256;

/// A physical keyboard position in the supported W3C UI Events `code` set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Key {
    /// The physical A position.
    KeyA,
    /// The physical B position.
    KeyB,
    /// The physical C position.
    KeyC,
    /// The physical D position.
    KeyD,
    /// The physical E position.
    KeyE,
    /// The physical F position.
    KeyF,
    /// The physical G position.
    KeyG,
    /// The physical H position.
    KeyH,
    /// The physical I position.
    KeyI,
    /// The physical J position.
    KeyJ,
    /// The physical K position.
    KeyK,
    /// The physical L position.
    KeyL,
    /// The physical M position.
    KeyM,
    /// The physical N position.
    KeyN,
    /// The physical O position.
    KeyO,
    /// The physical P position.
    KeyP,
    /// The physical Q position.
    KeyQ,
    /// The physical R position.
    KeyR,
    /// The physical S position.
    KeyS,
    /// The physical T position.
    KeyT,
    /// The physical U position.
    KeyU,
    /// The physical V position.
    KeyV,
    /// The physical W position.
    KeyW,
    /// The physical X position.
    KeyX,
    /// The physical Y position.
    KeyY,
    /// The physical Z position.
    KeyZ,
    /// The physical 0 position.
    Digit0,
    /// The physical 1 position.
    Digit1,
    /// The physical 2 position.
    Digit2,
    /// The physical 3 position.
    Digit3,
    /// The physical 4 position.
    Digit4,
    /// The physical 5 position.
    Digit5,
    /// The physical 6 position.
    Digit6,
    /// The physical 7 position.
    Digit7,
    /// The physical 8 position.
    Digit8,
    /// The physical 9 position.
    Digit9,
    /// The F1 key.
    F1,
    /// The F2 key.
    F2,
    /// The F3 key.
    F3,
    /// The F4 key.
    F4,
    /// The F5 key.
    F5,
    /// The F6 key.
    F6,
    /// The F7 key.
    F7,
    /// The F8 key.
    F8,
    /// The F9 key.
    F9,
    /// The F10 key.
    F10,
    /// The F11 key.
    F11,
    /// The F12 key.
    F12,
    /// The up arrow key.
    ArrowUp,
    /// The down arrow key.
    ArrowDown,
    /// The left arrow key.
    ArrowLeft,
    /// The right arrow key.
    ArrowRight,
    /// The Escape key.
    Escape,
    /// The space bar.
    Space,
    /// The Enter key.
    Enter,
    /// The Tab key.
    Tab,
    /// The Backspace key.
    Backspace,
    /// The Delete key.
    Delete,
    /// The Home key.
    Home,
    /// The End key.
    End,
    /// The `PageUp` key.
    PageUp,
    /// The `PageDown` key.
    PageDown,
    /// The minus position.
    Minus,
    /// The equal position.
    Equal,
    /// The left bracket position.
    BracketLeft,
    /// The right bracket position.
    BracketRight,
    /// The backslash position.
    Backslash,
    /// The semicolon position.
    Semicolon,
    /// The quote position.
    Quote,
    /// The comma position.
    Comma,
    /// The period position.
    Period,
    /// The slash position.
    Slash,
    /// The backquote position.
    Backquote,
    /// The left Shift key.
    ShiftLeft,
    /// The right Shift key.
    ShiftRight,
    /// The left Control key.
    ControlLeft,
    /// The right Control key.
    ControlRight,
    /// The left Alt key.
    AltLeft,
    /// The right Alt key.
    AltRight,
    /// The left Meta key.
    MetaLeft,
    /// The right Meta key.
    MetaRight,
}

impl Key {
    /// Every supported key exactly once, in stable bitset-index order.
    pub const ALL: [Self; KEY_COUNT] = [
        Self::KeyA,
        Self::KeyB,
        Self::KeyC,
        Self::KeyD,
        Self::KeyE,
        Self::KeyF,
        Self::KeyG,
        Self::KeyH,
        Self::KeyI,
        Self::KeyJ,
        Self::KeyK,
        Self::KeyL,
        Self::KeyM,
        Self::KeyN,
        Self::KeyO,
        Self::KeyP,
        Self::KeyQ,
        Self::KeyR,
        Self::KeyS,
        Self::KeyT,
        Self::KeyU,
        Self::KeyV,
        Self::KeyW,
        Self::KeyX,
        Self::KeyY,
        Self::KeyZ,
        Self::Digit0,
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::Digit5,
        Self::Digit6,
        Self::Digit7,
        Self::Digit8,
        Self::Digit9,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::Escape,
        Self::Space,
        Self::Enter,
        Self::Tab,
        Self::Backspace,
        Self::Delete,
        Self::Home,
        Self::End,
        Self::PageUp,
        Self::PageDown,
        Self::Minus,
        Self::Equal,
        Self::BracketLeft,
        Self::BracketRight,
        Self::Backslash,
        Self::Semicolon,
        Self::Quote,
        Self::Comma,
        Self::Period,
        Self::Slash,
        Self::Backquote,
        Self::ShiftLeft,
        Self::ShiftRight,
        Self::ControlLeft,
        Self::ControlRight,
        Self::AltLeft,
        Self::AltRight,
        Self::MetaLeft,
        Self::MetaRight,
    ];

    /// Returns this key's exact W3C `code` name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::KeyA => "KeyA",
            Self::KeyB => "KeyB",
            Self::KeyC => "KeyC",
            Self::KeyD => "KeyD",
            Self::KeyE => "KeyE",
            Self::KeyF => "KeyF",
            Self::KeyG => "KeyG",
            Self::KeyH => "KeyH",
            Self::KeyI => "KeyI",
            Self::KeyJ => "KeyJ",
            Self::KeyK => "KeyK",
            Self::KeyL => "KeyL",
            Self::KeyM => "KeyM",
            Self::KeyN => "KeyN",
            Self::KeyO => "KeyO",
            Self::KeyP => "KeyP",
            Self::KeyQ => "KeyQ",
            Self::KeyR => "KeyR",
            Self::KeyS => "KeyS",
            Self::KeyT => "KeyT",
            Self::KeyU => "KeyU",
            Self::KeyV => "KeyV",
            Self::KeyW => "KeyW",
            Self::KeyX => "KeyX",
            Self::KeyY => "KeyY",
            Self::KeyZ => "KeyZ",
            Self::Digit0 => "Digit0",
            Self::Digit1 => "Digit1",
            Self::Digit2 => "Digit2",
            Self::Digit3 => "Digit3",
            Self::Digit4 => "Digit4",
            Self::Digit5 => "Digit5",
            Self::Digit6 => "Digit6",
            Self::Digit7 => "Digit7",
            Self::Digit8 => "Digit8",
            Self::Digit9 => "Digit9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::ArrowUp => "ArrowUp",
            Self::ArrowDown => "ArrowDown",
            Self::ArrowLeft => "ArrowLeft",
            Self::ArrowRight => "ArrowRight",
            Self::Escape => "Escape",
            Self::Space => "Space",
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Minus => "Minus",
            Self::Equal => "Equal",
            Self::BracketLeft => "BracketLeft",
            Self::BracketRight => "BracketRight",
            Self::Backslash => "Backslash",
            Self::Semicolon => "Semicolon",
            Self::Quote => "Quote",
            Self::Comma => "Comma",
            Self::Period => "Period",
            Self::Slash => "Slash",
            Self::Backquote => "Backquote",
            Self::ShiftLeft => "ShiftLeft",
            Self::ShiftRight => "ShiftRight",
            Self::ControlLeft => "ControlLeft",
            Self::ControlRight => "ControlRight",
            Self::AltLeft => "AltLeft",
            Self::AltRight => "AltRight",
            Self::MetaLeft => "MetaLeft",
            Self::MetaRight => "MetaRight",
        }
    }

    /// Parses an exact, case-sensitive W3C `code` name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.name() == name)
    }

    const fn index(self) -> usize {
        self as usize
    }

    const fn modifier(self) -> Option<u8> {
        match self {
            Self::ControlLeft | Self::ControlRight => Some(CONTROL),
            Self::AltLeft | Self::AltRight => Some(ALT),
            Self::ShiftLeft | Self::ShiftRight => Some(SHIFT),
            Self::MetaLeft | Self::MetaRight => Some(META),
            _ => None,
        }
    }
}

const CONTROL: u8 = 1;
const ALT: u8 = 2;
const SHIFT: u8 = 4;
const META: u8 = 8;
const DEFAULT_ACTIONS: [(&str, &[&str]); 6] = [
    ("move_left", &["KeyA", "ArrowLeft"]),
    ("move_right", &["KeyD", "ArrowRight"]),
    ("move_up", &["KeyW", "ArrowUp"]),
    ("move_down", &["KeyS", "ArrowDown"]),
    ("close", &["Escape"]),
    ("reload_texture", &["KeyR"]),
];

/// A validated physical-key binding and its required modifier prefixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    key: Key,
    modifiers: u8,
}

impl Binding {
    /// Returns the binding's main physical key.
    #[must_use]
    pub const fn key(self) -> Key {
        self.key
    }

    /// Returns whether Control must be held by either side.
    #[must_use]
    pub const fn requires_control(self) -> bool {
        self.modifiers & CONTROL != 0
    }

    /// Returns whether Alt must be held by either side.
    #[must_use]
    pub const fn requires_alt(self) -> bool {
        self.modifiers & ALT != 0
    }

    /// Returns whether Shift must be held by either side.
    #[must_use]
    pub const fn requires_shift(self) -> bool {
        self.modifiers & SHIFT != 0
    }

    /// Returns whether Meta must be held by either side.
    #[must_use]
    pub const fn requires_meta(self) -> bool {
        self.modifiers & META != 0
    }
}

/// One action and its immutable, validated bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionBindings {
    name: String,
    bindings: Vec<Binding>,
}

impl ActionBindings {
    /// The action name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The action's bindings in the order supplied by its caller.
    #[must_use]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
}

/// A complete, sorted set of action bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    actions: Vec<ActionBindings>,
}

impl Bindings {
    /// Validates explicit action entries and fills each missing design default.
    ///
    /// An explicit action with an empty list is retained and therefore unbound.
    ///
    /// # Errors
    ///
    /// Returns an error naming the action and offending entry for invalid names,
    /// duplicates, malformed bindings, or finite-limit violations.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Result<Self, BindingsError> {
        let mut actions = DEFAULT_ACTIONS
            .iter()
            .map(|(name, values)| ActionBindings {
                name: (*name).to_owned(),
                bindings: values
                    .iter()
                    .filter_map(|value| parse_binding(value).ok())
                    .collect(),
            })
            .collect::<Vec<_>>();
        let mut seen = Vec::<String>::with_capacity(64);

        for (name, entries) in entries {
            if !valid_action_name(&name) {
                return Err(BindingsError::new(
                    &name,
                    &name,
                    "action name must match [a-z][a-z0-9_]* and be at most 32 bytes",
                ));
            }
            if seen.iter().any(|prior| prior == &name) {
                return Err(BindingsError::new(
                    &name,
                    &name,
                    "action is given more than once",
                ));
            }
            seen.push(name.clone());

            let index = if let Some(index) = actions.iter().position(|action| action.name == name) {
                index
            } else {
                if actions.len() == 64 {
                    return Err(BindingsError::new(
                        &name,
                        &name,
                        "at most 64 actions are allowed",
                    ));
                }
                actions.push(ActionBindings {
                    name: name.clone(),
                    bindings: Vec::new(),
                });
                actions.len() - 1
            };

            if entries.len() > 8 {
                return Err(BindingsError::new(
                    &name,
                    &entries[8],
                    "at most 8 bindings are allowed for one action",
                ));
            }
            let mut parsed = Vec::with_capacity(entries.len());
            for entry in entries {
                let binding = parse_binding(&entry)
                    .map_err(|reason| BindingsError::new(&name, &entry, reason))?;
                if parsed.contains(&binding) {
                    return Err(BindingsError::new(
                        &name,
                        &entry,
                        "the same binding is listed twice for this action",
                    ));
                }
                parsed.push(binding);
            }
            actions[index].bindings = parsed;
        }

        actions.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(Self { actions })
    }

    /// Returns all actions in stable alphabetical order.
    #[must_use]
    pub fn actions(&self) -> &[ActionBindings] {
        &self.actions
    }

    /// Returns one action by exact name, if present.
    #[must_use]
    pub fn action(&self, name: &str) -> Option<&ActionBindings> {
        self.actions
            .binary_search_by(|action| action.name.as_str().cmp(name))
            .ok()
            .map(|index| &self.actions[index])
    }
}

/// A binding validation error that identifies its action and input entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingsError {
    message: String,
}

impl BindingsError {
    fn new(action: &str, entry: &str, reason: &str) -> Self {
        Self {
            message: format!("action {action:?}, entry {entry:?}: {reason}"),
        }
    }
}

impl fmt::Display for BindingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BindingsError {}

fn valid_action_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn parse_binding(entry: &str) -> Result<Binding, &'static str> {
    if entry.is_empty() {
        return Err("binding is empty");
    }
    if entry.len() > 64 {
        return Err("binding exceeds 64 bytes");
    }
    if entry.chars().any(char::is_whitespace) {
        return Err("binding contains whitespace");
    }

    let mut parts = entry.split('+').peekable();
    let mut modifiers = 0_u8;
    let mut previous_rank = 0_u8;
    let mut main = None;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            main = Some(part);
            break;
        }
        let (rank, flag) = match part {
            "Control" => (1, CONTROL),
            "Alt" => (2, ALT),
            "Shift" => (3, SHIFT),
            "Meta" => (4, META),
            _ => return Err("unknown modifier prefix"),
        };
        if modifiers & flag != 0 {
            return Err("modifier prefix is repeated");
        }
        if rank < previous_rank {
            return Err("modifier prefixes are out of order");
        }
        modifiers |= flag;
        previous_rank = rank;
    }

    let key_name = main.ok_or("binding has no main key")?;
    let key = Key::from_name(key_name).ok_or("unknown physical key name")?;
    if modifiers != 0 && key.modifier().is_some() {
        return Err("a prefixed binding cannot use a modifier as its main key");
    }
    Ok(Binding { key, modifiers })
}

/// The kind of action transition recorded in a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// The action became held.
    Press,
    /// The action stopped being held.
    Release,
}

/// One ordered action edge in an input snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// The action name.
    pub action: String,
    /// The edge direction.
    pub edge: Edge,
}

/// One action's coherent state in an input snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSnapshot {
    /// The action name.
    pub name: String,
    /// Whether any binding is currently satisfied.
    pub held: bool,
    /// Whether a press occurred since the last acknowledged snapshot.
    pub pressed: bool,
    /// Whether a release occurred since the last acknowledged snapshot.
    pub released: bool,
}

/// An immutable copy of the mapper state at one sequence watermark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSnapshot {
    /// Monotonic snapshot identifier used by [`Mapper::acknowledge`].
    pub sequence: u128,
    /// Per-action held state and latched edges, sorted by action name.
    pub actions: Vec<ActionSnapshot>,
    /// Ordered transition detail still retained by the bounded ring.
    pub transitions: Vec<Transition>,
    /// Whether transition detail has overflowed since the last acknowledgement.
    pub overflow: bool,
    /// Whether focus loss occurred since the last acknowledgement.
    pub reset: bool,
    /// Cumulative, saturating count of discarded transition records.
    pub overflow_count: u64,
}

/// The final physical state of every supported key, with the taps a native layer could not deliver in order, as one
/// resynchronisation input for [`Mapper::resync`] (`docs/scene-design.md` §10.2).
///
/// `held` is exact: the keys down now. `pressed` and `released` are the per-key **loss latches**: a key is in `pressed`
/// if a press of it was in a discarded transition, in `released` if a release was. They are evidence of taps, never of
/// order. One bit per key in each of three sets, in fixed storage; bits beyond [`KEY_COUNT`] are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyStates {
    held: [u64; 2],
    pressed: [u64; 2],
    released: [u64; 2],
}

impl KeyStates {
    /// Every key up and no latch set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            held: [0; 2],
            pressed: [0; 2],
            released: [0; 2],
        }
    }

    /// The three sets as the words a native layer keeps them in (bit `n % 64` of word `n / 64` is the key whose
    /// position in [`Key::ALL`] is `n`); bits beyond the supported keys are cleared.
    #[must_use]
    pub const fn from_words(held: [u64; 2], pressed: [u64; 2], released: [u64; 2]) -> Self {
        let mask = [u64::MAX, (1_u64 << (KEY_COUNT - 64)) - 1];
        Self {
            held: [held[0] & mask[0], held[1] & mask[1]],
            pressed: [pressed[0] & mask[0], pressed[1] & mask[1]],
            released: [released[0] & mask[0], released[1] & mask[1]],
        }
    }

    /// Marks `key` held now.
    pub fn set_held(&mut self, key: Key) {
        Self::set(&mut self.held, key);
    }

    /// Marks `key` as pressed in a discarded transition.
    pub fn set_pressed(&mut self, key: Key) {
        Self::set(&mut self.pressed, key);
    }

    /// Marks `key` as released in a discarded transition.
    pub fn set_released(&mut self, key: Key) {
        Self::set(&mut self.released, key);
    }

    /// Whether `key` is held now.
    #[must_use]
    pub const fn is_held(&self, key: Key) -> bool {
        Self::get(&self.held, key)
    }

    /// Whether a press of `key` was in a discarded transition.
    #[must_use]
    pub const fn was_pressed(&self, key: Key) -> bool {
        Self::get(&self.pressed, key)
    }

    /// Whether a release of `key` was in a discarded transition.
    #[must_use]
    pub const fn was_released(&self, key: Key) -> bool {
        Self::get(&self.released, key)
    }

    const fn get(words: &[u64; 2], key: Key) -> bool {
        let index = key.index();
        words[index / 64] & (1_u64 << (index % 64)) != 0
    }

    fn set(words: &mut [u64; 2], key: Key) {
        let index = key.index();
        words[index / 64] |= 1_u64 << (index % 64);
    }
}

#[derive(Clone, Copy, Debug)]
struct TransitionRecord {
    action: usize,
    edge: Edge,
    sequence: u128,
}

#[derive(Clone, Copy, Debug, Default)]
struct ActionState {
    held: bool,
    pressed_sequence: Option<u128>,
    released_sequence: Option<u128>,
}

/// The single-owner mapper for physical keys and named actions.
///
/// The action state and 256-entry transition ring are allocated at construction. [`Mapper::key`]
/// and [`Mapper::focus_lost`] do not allocate. [`Mapper::resync`] replaces the key bitset and recomputes the actions
/// from that final set, for a native layer that lost ordered detail.
pub struct Mapper {
    bindings: Bindings,
    keys: [u64; 2],
    actions: Vec<ActionState>,
    transitions: [Option<TransitionRecord>; TRANSITION_CAPACITY],
    transition_head: usize,
    transition_len: usize,
    sequence: u128,
    acknowledged_sequence: u128,
    overflow_sequence: Option<u128>,
    reset_sequence: Option<u128>,
    overflow_count: u64,
    close_presses: u32,
}

impl Mapper {
    /// Creates a mapper with all keys up and the given validated bindings.
    #[must_use]
    pub fn new(bindings: Bindings) -> Self {
        let actions = vec![ActionState::default(); bindings.actions.len()];
        Self {
            bindings,
            keys: [0; 2],
            actions,
            transitions: [None; TRANSITION_CAPACITY],
            transition_head: 0,
            transition_len: 0,
            sequence: 0,
            acknowledged_sequence: 0,
            overflow_sequence: None,
            reset_sequence: None,
            overflow_count: 0,
            close_presses: 0,
        }
    }

    /// Applies one physical key transition. Repeats of a held key do nothing; a repeated down
    /// for an unheld key is treated as a press to recover from a missed initial event.
    pub fn key(&mut self, key: Key, down: bool, repeat: bool) {
        let was_down = self.key_is_down(key);
        if repeat && down && was_down {
            return;
        }
        if was_down == down {
            return;
        }

        self.set_key(key, down);
        self.recompute_actions();
    }

    /// Clears all held keys and records a release edge for every action that was held.
    ///
    /// `reset` remains latched until its reporting snapshot is acknowledged.
    pub fn focus_lost(&mut self) {
        self.keys = [0; 2];
        self.recompute_actions();
        self.reset_sequence = Some(self.next_sequence());
    }

    /// Resynchronises with the final physical key states of a native layer that discarded ordered detail, in one step
    /// and with no ordered event (`docs/scene-design.md` §10.2).
    ///
    /// - **Held state, from the final key states only.** The mapper's key set becomes `states.held` and every
    ///   action's held state is recomputed from it, independent of what the mapper believed before: an action held
    ///   now and not before gets a press edge, one held before and not now a release edge. A prefixed binding is held
    ///   here **only** when its main key and a key of each of its prefixes are physically held now.
    /// - **Lost taps, unprefixed bindings only.** A key whose latch shows a press latches `pressed` (and `released` if
    ///   the key is up now) on every action that has that key as an **unprefixed** binding (an unprefixed binding fires
    ///   whatever modifiers are held, so this is true whatever the lost order was). A prefixed binding never gets a
    ///   press or a tap from the latches: latches and final modifier states do not prove that a chord overlapped, so a
    ///   prefixed chord tapped inside the lost span is lost, and is never invented.
    /// - **Close.** The close-press counter gains at most one press per resync, when the `close` action got a press by
    ///   either rule.
    /// - The snapshot reports `overflow`, and the overflow count grows by one (a resync is one loss of detail).
    pub fn resync(&mut self, states: &KeyStates) {
        self.keys = states.held;
        let mut close_pressed = false;
        for index in 0..self.bindings.actions.len() {
            let now_held = self.bindings.actions[index]
                .bindings
                .iter()
                .copied()
                .any(|binding| self.binding_is_held(binding));
            let was_held = self.actions[index].held;
            let mut pressed = now_held && !was_held;
            if now_held != was_held {
                self.actions[index].held = now_held;
                let edge = if now_held { Edge::Press } else { Edge::Release };
                let sequence = self.next_sequence();
                if now_held {
                    self.actions[index].pressed_sequence = Some(sequence);
                } else {
                    self.actions[index].released_sequence = Some(sequence);
                }
                self.push_transition(TransitionRecord {
                    action: index,
                    edge,
                    sequence,
                });
            }
            // A tap lost inside the discarded span, on an unprefixed binding of this action.
            for position in 0..self.bindings.actions[index].bindings.len() {
                let binding = self.bindings.actions[index].bindings[position];
                if binding.modifiers != 0 || !states.was_pressed(binding.key) {
                    continue;
                }
                // Every latched edge is stamped with the sequence of this resync, even when an older, unacknowledged
                // edge of the action is still pending: an acknowledgement of a snapshot taken before the resync must
                // not clear the tap the resync latched.
                let sequence = self.next_sequence();
                self.actions[index].pressed_sequence = Some(sequence);
                pressed = true;
                if !states.is_held(binding.key) {
                    self.actions[index].released_sequence = Some(sequence);
                }
            }
            if pressed && self.bindings.actions[index].name == "close" {
                close_pressed = true;
            }
        }
        if close_pressed {
            self.close_presses = self.close_presses.saturating_add(1);
        }
        self.overflow_sequence = Some(self.next_sequence());
        self.overflow_count = self.overflow_count.saturating_add(1);
    }

    /// Returns a coherent snapshot without consuming any edge or transition.
    #[must_use]
    pub fn snapshot(&mut self) -> InputSnapshot {
        self.sequence = self.sequence.wrapping_add(1);
        let sequence = self.sequence;
        let actions = self
            .bindings
            .actions
            .iter()
            .zip(&self.actions)
            .map(|(binding, state)| ActionSnapshot {
                name: binding.name.clone(),
                held: state.held,
                pressed: state.pressed_sequence.is_some(),
                released: state.released_sequence.is_some(),
            })
            .collect();
        let transitions = (0..self.transition_len)
            .filter_map(|offset| {
                let index = (self.transition_head + offset) % TRANSITION_CAPACITY;
                self.transitions[index]
            })
            .map(|record| Transition {
                action: self.bindings.actions[record.action].name.clone(),
                edge: record.edge,
            })
            .collect();
        InputSnapshot {
            sequence,
            actions,
            transitions,
            overflow: self.overflow_sequence.is_some(),
            reset: self.reset_sequence.is_some(),
            overflow_count: self.overflow_count,
        }
    }

    /// Acknowledges exactly the events no newer than a previously issued snapshot.
    ///
    /// An older, unknown, future, or already acknowledged sequence changes nothing.
    pub fn acknowledge(&mut self, sequence: u128) {
        if sequence == 0 || sequence > self.sequence || sequence <= self.acknowledged_sequence {
            return;
        }
        self.acknowledged_sequence = sequence;
        for state in &mut self.actions {
            if state
                .pressed_sequence
                .is_some_and(|stamp| stamp <= sequence)
            {
                state.pressed_sequence = None;
            }
            if state
                .released_sequence
                .is_some_and(|stamp| stamp <= sequence)
            {
                state.released_sequence = None;
            }
        }
        if self
            .overflow_sequence
            .is_some_and(|stamp| stamp <= sequence)
        {
            self.overflow_sequence = None;
        }
        if self.reset_sequence.is_some_and(|stamp| stamp <= sequence) {
            self.reset_sequence = None;
        }
        while self.transition_len > 0 {
            let Some(front) = self.transitions[self.transition_head] else {
                break;
            };
            if front.sequence > sequence {
                break;
            }
            self.transitions[self.transition_head] = None;
            self.transition_head = (self.transition_head + 1) % TRANSITION_CAPACITY;
            self.transition_len -= 1;
        }
    }

    /// Drains the saturating, lossless count of close-action presses.
    #[must_use]
    pub fn take_close_presses(&mut self) -> u32 {
        std::mem::take(&mut self.close_presses)
    }

    /// The cumulative saturating count of transition records discarded from the ring.
    #[must_use]
    pub const fn overflow_count(&self) -> u64 {
        self.overflow_count
    }

    /// The immutable binding configuration used by this mapper.
    #[must_use]
    pub const fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    fn next_sequence(&self) -> u128 {
        self.sequence.wrapping_add(1)
    }

    fn key_is_down(&self, key: Key) -> bool {
        let index = key.index();
        self.keys[index / 64] & (1_u64 << (index % 64)) != 0
    }

    fn set_key(&mut self, key: Key, down: bool) {
        let index = key.index();
        let bit = 1_u64 << (index % 64);
        if down {
            self.keys[index / 64] |= bit;
        } else {
            self.keys[index / 64] &= !bit;
        }
    }

    fn modifier_is_down(&self, modifier: u8) -> bool {
        match modifier {
            CONTROL => self.key_is_down(Key::ControlLeft) || self.key_is_down(Key::ControlRight),
            ALT => self.key_is_down(Key::AltLeft) || self.key_is_down(Key::AltRight),
            SHIFT => self.key_is_down(Key::ShiftLeft) || self.key_is_down(Key::ShiftRight),
            META => self.key_is_down(Key::MetaLeft) || self.key_is_down(Key::MetaRight),
            _ => false,
        }
    }

    fn binding_is_held(&self, binding: Binding) -> bool {
        self.key_is_down(binding.key)
            && (binding.modifiers & CONTROL == 0 || self.modifier_is_down(CONTROL))
            && (binding.modifiers & ALT == 0 || self.modifier_is_down(ALT))
            && (binding.modifiers & SHIFT == 0 || self.modifier_is_down(SHIFT))
            && (binding.modifiers & META == 0 || self.modifier_is_down(META))
    }

    fn recompute_actions(&mut self) {
        for index in 0..self.bindings.actions.len() {
            let now_held = self.bindings.actions[index]
                .bindings
                .iter()
                .copied()
                .any(|binding| self.binding_is_held(binding));
            let was_held = self.actions[index].held;
            if now_held != was_held {
                self.actions[index].held = now_held;
                let edge = if now_held { Edge::Press } else { Edge::Release };
                let sequence = self.next_sequence();
                if now_held {
                    self.actions[index].pressed_sequence = Some(sequence);
                    if self.bindings.actions[index].name == "close" {
                        self.close_presses = self.close_presses.saturating_add(1);
                    }
                } else {
                    self.actions[index].released_sequence = Some(sequence);
                }
                self.push_transition(TransitionRecord {
                    action: index,
                    edge,
                    sequence,
                });
            }
        }
    }

    fn push_transition(&mut self, record: TransitionRecord) {
        if self.transition_len == TRANSITION_CAPACITY {
            self.transitions[self.transition_head] = None;
            self.transition_head = (self.transition_head + 1) % TRANSITION_CAPACITY;
            self.transition_len -= 1;
            self.overflow_sequence = Some(record.sequence);
            self.overflow_count = self.overflow_count.saturating_add(1);
        }
        let tail = (self.transition_head + self.transition_len) % TRANSITION_CAPACITY;
        self.transitions[tail] = Some(record);
        self.transition_len += 1;
    }
}

#[cfg(test)]
mod tests;
