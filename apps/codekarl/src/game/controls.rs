//! The controls: the input actions that pause, change the speed, save and load, and how a snapshot's presses become them.

use std::sync::mpsc::Sender;

use ck_window::{Edge, InputSnapshot};

use super::core::Message;

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
}

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

/// The main thread's end of the control channel.
#[derive(Clone)]
pub(crate) struct SimControls {
    sender: Sender<Message>,
}

impl SimControls {
    pub(crate) const fn new(sender: Sender<Message>) -> Self {
        Self { sender }
    }

    /// Sends every press of `input` to the simulation owner, in order, and says whether the input layer discarded earlier
    /// transitions (the caller writes the one record). A press sent after the owner ended is dropped.
    pub(crate) fn send(&self, input: &InputSnapshot) -> bool {
        for control in presses(input) {
            let _ = self.sender.send(Message::Control(control));
        }
        input.overflow
    }
}
