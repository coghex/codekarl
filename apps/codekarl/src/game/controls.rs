//! The controls: the input actions that pause, change the speed, save and load, and how a snapshot's presses become them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use ck_window::{Edge, InputSnapshot};

use super::core::{Message, SaveCall, SaveReply};

/// The record written (once per update) when the input layer discarded earlier transitions.
pub(crate) const OVERFLOW_RECORD: &str = "input overflow; earlier time presses were lost";

/// What the player asked of the simulation. A control is not a command: it is not state and is not journalled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    /// `time_pause`: running pauses; paused resumes.
    TogglePause,
    /// `time_speed_1` to `time_speed_5`: the speed level.
    Speed(u8),
    /// `time_faster`: one speed up.
    Faster,
    /// `time_slower`: one speed down.
    Slower,
    /// `game_quicksave`.
    Quicksave,
    /// `game_quickload`.
    Quickload,
    /// The pause menu's pause: running pauses with the reason `player`; a game that is already paused stays as it is, reason
    /// included. No input action maps to it ([`control_of`] never returns it): only `game.pause()` sends it.
    Pause,
}

/// The game's own key actions: what the pause menu stops while it is open (`game.set_modal`). The block names these and nothing
/// else, so `close` (Escape) and every other action, a later key-driven tool's included, still arrive.
pub(crate) const BLOCKED_ACTIONS: [&str; 16] = [
    "time_pause",
    "time_speed_1",
    "time_speed_2",
    "time_speed_3",
    "time_speed_4",
    "time_speed_5",
    "time_faster",
    "time_slower",
    "game_quicksave",
    "game_quickload",
    "map_pan_left",
    "map_pan_right",
    "map_pan_up",
    "map_pan_down",
    "map_zoom_in",
    "map_zoom_out",
];

/// The control an input action name stands for, if it is one of the time and save actions.
pub(crate) fn control_of(action: &str) -> Option<Control> {
    Some(match action {
        "time_pause" => Control::TogglePause,
        "time_speed_1" => Control::Speed(1),
        "time_speed_2" => Control::Speed(2),
        "time_speed_3" => Control::Speed(3),
        "time_speed_4" => Control::Speed(4),
        "time_speed_5" => Control::Speed(5),
        "time_faster" => Control::Faster,
        "time_slower" => Control::Slower,
        "game_quicksave" => Control::Quicksave,
        "game_quickload" => Control::Quickload,
        _ => return None,
    })
}

/// The presses of a snapshot, in the order the snapshot retains them.
///
/// They come from the ordered transitions, not from the per-action latches: two taps between updates collapse to one latch,
/// and the order across actions is lost.
pub(crate) fn presses(input: &InputSnapshot) -> Vec<Control> {
    input
        .transitions
        .iter()
        .filter(|transition| transition.edge == Edge::Press)
        .filter_map(|transition| control_of(&transition.action))
        .collect()
}

/// Whether `action` is one the pause menu stops while it is open.
pub(crate) fn is_blocked_action(action: &str) -> bool {
    BLOCKED_ACTIONS.contains(&action)
}

/// `input` as the map and the simulation see it while the key block is on: the blocked actions have no transitions, are not held
/// and have no press or release latch. Everything else, `close` and the other actions included, is kept. Filtering the held and
/// latched state as well as the ordered transitions matters: the map reads held actions (pan and zoom), the simulation reads
/// ordered presses.
pub(crate) fn without_blocked_actions(input: &InputSnapshot) -> InputSnapshot {
    let mut filtered = input.clone();
    filtered
        .transitions
        .retain(|transition| !is_blocked_action(&transition.action));
    for action in &mut filtered.actions {
        if is_blocked_action(&action.name) {
            action.held = false;
            action.pressed = false;
            action.released = false;
        }
    }
    filtered
}

/// The main thread's end of the control channel, and the key block the pause menu turns on and off through `game.set_modal`.
/// Clones share both.
#[derive(Clone)]
pub(crate) struct SimControls {
    sender: Sender<Message>,
    block: Arc<AtomicBool>,
}

impl SimControls {
    pub(crate) fn new(sender: Sender<Message>) -> Self {
        Self {
            sender,
            block: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Turns the game-key block on or off. Off at the start of a run.
    pub(crate) fn set_keys_blocked(&self, blocked: bool) {
        self.block.store(blocked, Ordering::SeqCst);
    }

    /// Whether the game-key block is on.
    pub(crate) fn keys_blocked(&self) -> bool {
        self.block.load(Ordering::SeqCst)
    }

    /// Sends every press of `input` to the simulation owner, in order, and says whether the input layer discarded earlier
    /// transitions (the caller writes the one record). A press sent after the owner ended is dropped.
    pub(crate) fn send(&self, input: &InputSnapshot) -> bool {
        for control in presses(input) {
            self.send_control(control);
        }
        input.overflow
    }

    /// Sends one control through the same channel used by key presses. A control after the simulation owner ended is dropped.
    pub(crate) fn send_control(&self, control: Control) {
        let _ = self.sender.send(Message::Control(control));
    }

    /// Runs a named-save operation on the simulation owner and waits for its completed result.
    pub(crate) fn save_call(&self, call: SaveCall) -> Result<SaveReply, String> {
        let (reply, result) = std::sync::mpsc::channel();
        self.sender
            .send(Message::Save(call, reply))
            .map_err(|_| "the game is no longer running".to_owned())?;
        result
            .recv()
            .map_err(|_| "the game ended before the save operation finished".to_owned())
    }
}
