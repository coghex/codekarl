//! The time and save presses, from the input snapshot's ordered transitions through the UI link to the simulation owner.

use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use ck_lua::HostFunction;
use ck_window::{
    ActionSnapshot, Edge, InputSnapshot, InputTick, PixelSize, PostError, Transition, UiEvent,
    UiLink, UpdateEvent, WindowMetrics,
};

use super::Harness as CoreHarness;
use crate::{
    game::{
        controls::{Control, OVERFLOW_RECORD, SimControls, control_of, presses},
        core::Message,
        slot::{PauseReason, SimSlot},
    },
    testing::{Extras, Gate, Harness, WINDOW},
    ui_link::SimLink,
};

fn transition(action: &str, edge: Edge) -> Transition {
    Transition {
        action: action.to_owned(),
        edge,
    }
}

fn press(action: &str) -> Transition {
    transition(action, Edge::Press)
}

/// A snapshot with `transitions` in order. Its per-action latches say every named action was pressed: they carry no order.
fn snapshot(transitions: Vec<Transition>, overflow: bool) -> InputSnapshot {
    let mut names: Vec<_> = transitions.iter().map(|t| t.action.clone()).collect();
    names.sort();
    names.dedup();
    InputSnapshot {
        sequence: 1,
        actions: names
            .into_iter()
            .map(|name| ActionSnapshot {
                name,
                held: false,
                pressed: true,
                released: false,
            })
            .collect(),
        transitions,
        overflow,
        reset: false,
        overflow_count: u64::from(overflow),
        pointer_events: Vec::new(),
        pointer_position: None,
        pointer_held: false,
        pointer_overflow: false,
    }
}

fn update(sequence: u64) -> UiEvent {
    UiEvent::Update(UpdateEvent {
        sequence,
        elapsed: Duration::from_millis(16),
        dt: Duration::from_millis(16),
        metrics: metrics(),
    })
}

fn metrics() -> WindowMetrics {
    WindowMetrics {
        points: (100, 80),
        pixels: PixelSize {
            width: 200,
            height: 160,
        },
    }
}

/// The input tick that gathered `input`: the time controls act on a press at its tick, not at the UI update.
fn tick(input: InputSnapshot) -> InputTick {
    InputTick {
        dt: Duration::from_millis(16),
        metrics: metrics(),
        input,
    }
}

/// A UI link with a simulation attached, and the receiving end of its control channel.
fn linked(harness: &Harness) -> (crate::ui_link::CodekarlUi, mpsc::Receiver<Message>) {
    let (sender, receiver) = mpsc::channel();
    let link = harness.ui.clone().with_sim(SimLink {
        controls: SimControls::new(sender),
        slot: SimSlot::new(Arc::new(|| {})),
        time_registered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        hover_registered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    (link, receiver)
}

fn received(receiver: &mpsc::Receiver<Message>) -> Vec<Message> {
    receiver.try_iter().collect()
}

#[test]
fn the_ten_actions_are_the_controls() {
    for (action, control) in [
        ("time_pause", Control::TogglePause),
        ("time_speed_1", Control::Speed(1)),
        ("time_speed_2", Control::Speed(2)),
        ("time_speed_3", Control::Speed(3)),
        ("time_speed_4", Control::Speed(4)),
        ("time_speed_5", Control::Speed(5)),
        ("time_faster", Control::Faster),
        ("time_slower", Control::Slower),
        ("game_quicksave", Control::Quicksave),
        ("game_quickload", Control::Quickload),
    ] {
        assert_eq!(control_of(action), Some(control), "{action}");
    }
    assert_eq!(control_of("map_pan_left"), None);
    assert_eq!(control_of("time_speed_6"), None);
    assert_eq!(control_of(""), None);
}

#[test]
fn only_presses_of_these_actions_count_and_in_the_order_of_the_transitions() {
    let input = snapshot(
        vec![
            press("time_speed_3"),
            press("map_pan_left"),
            transition("time_speed_3", Edge::Release),
            press("time_pause"),
            transition("time_pause", Edge::Release),
        ],
        false,
    );
    assert_eq!(presses(&input), [Control::Speed(3), Control::TogglePause]);
    // The latches are not the source: an action latched as pressed with no transition is no press.
    let mut latched = snapshot(vec![press("time_pause")], false);
    latched.transitions.clear();
    assert!(latched.actions.iter().any(|action| action.pressed));
    assert!(presses(&latched).is_empty());
}

#[test]
fn two_space_presses_in_one_snapshot_toggle_twice() {
    let harness = Harness::new("", Extras::default());
    let (mut link, receiver) = linked(&harness);
    let input = snapshot(
        vec![
            press("time_pause"),
            transition("time_pause", Edge::Release),
            press("time_pause"),
            transition("time_pause", Edge::Release),
        ],
        false,
    );
    link.input(&tick(input)).expect("the tick");
    let messages = received(&receiver);
    assert_eq!(
        messages,
        [
            Message::Control(Control::TogglePause),
            Message::Control(Control::TogglePause)
        ]
    );
    // Applied to a game that is running: paused, then running.
    let mut game = CoreHarness::new(1);
    game.run_at(1);
    let mut states = Vec::new();
    for message in messages {
        game.core.dispatch(message, game.now);
        states.push(game.paused());
    }
    assert_eq!(states, [Some(PauseReason::Player), None]);
}

#[test]
fn a_speed_then_pause_keeps_its_order_across_actions() {
    let harness = Harness::new("", Extras::default());
    let (mut link, receiver) = linked(&harness);
    link.input(&tick(snapshot(
        vec![press("time_speed_3"), press("time_pause")],
        false,
    )))
    .expect("the tick");
    assert_eq!(
        received(&receiver),
        [
            Message::Control(Control::Speed(3)),
            Message::Control(Control::TogglePause)
        ]
    );
    // The order matters to the result: speed 2 then faster is 3, faster then speed 2 is 2.
    for (controls, level) in [
        ([Control::Speed(2), Control::Faster], 3),
        ([Control::Faster, Control::Speed(2)], 2),
    ] {
        let mut game = CoreHarness::new(1);
        for control in controls {
            game.control(control);
        }
        assert_eq!(game.core.snapshot().speed.level(), level);
    }
}

#[test]
fn a_press_acts_once_at_its_tick_whatever_happens_to_the_update_that_follows() {
    let gate = Gate::new();
    let hold: HostFunction = {
        let gate = Arc::clone(&gate);
        Arc::new(move |_| {
            gate.wait();
            Ok(Vec::new())
        })
    };
    let harness = Harness::new(
        &format!("{WINDOW}\nwindow.on_opened(function() window.hold() end)"),
        Extras {
            functions: vec![("hold", hold)],
            ..Extras::default()
        },
    );
    let (mut link, receiver) = linked(&harness);
    // Three calls in flight leave one slot, reserved for lifecycle posts: an update is refused.
    for _ in 0..3 {
        link.post(UiEvent::Opened).expect("within the limit");
    }
    // The presses act at the tick, with the UI full and the update refused (independent of the UI VM).
    let first = snapshot(vec![press("time_speed_4"), press("time_pause")], false);
    link.input(&tick(first)).expect("the tick");
    assert_eq!(link.post(update(1)), Err(PostError::Full));
    assert_eq!(
        received(&receiver),
        [
            Message::Control(Control::Speed(4)),
            Message::Control(Control::TogglePause),
        ],
        "the presses acted at their tick, though the update was refused"
    );

    // The loop acknowledged the tick, so the next snapshot holds only what is new; the retried post repeats nothing.
    gate.open();
    for _ in 0..3 {
        while link.take_answer().is_none() {
            std::thread::yield_now();
        }
    }
    link.input(&tick(snapshot(vec![press("game_quicksave")], false)))
        .expect("the tick");
    link.post(update(2)).expect("accepted");
    assert_eq!(
        received(&receiver),
        [Message::Control(Control::Quicksave)],
        "only the new press acted; the accepted post acted on nothing"
    );
    while link.take_answer().is_none() {
        std::thread::yield_now();
    }
    link.input(&tick(snapshot(Vec::new(), false)))
        .expect("the tick");
    link.post(update(3)).expect("accepted");
    assert!(received(&receiver).is_empty());
}

#[test]
fn an_overflowed_snapshot_applies_its_retained_presses_and_reports_the_loss_once() {
    let harness = Harness::new("", Extras::default());
    let (sender, receiver) = mpsc::channel();
    let controls = SimControls::new(sender);
    // The input layer discarded the oldest transitions: only the retained presses are applied, none is inferred.
    let input = snapshot(vec![press("time_slower"), press("time_pause")], true);
    assert!(controls.send(&input), "the loss is reported");
    assert_eq!(
        received(&receiver),
        [
            Message::Control(Control::Slower),
            Message::Control(Control::TogglePause)
        ]
    );
    assert!(!controls.send(&snapshot(vec![press("time_pause")], false)));
    assert_eq!(
        OVERFLOW_RECORD,
        "input overflow; earlier time presses were lost"
    );
    drop(harness);
}

#[test]
fn presses_sent_after_the_owner_ended_are_dropped() {
    let (sender, receiver) = mpsc::channel();
    drop(receiver);
    let controls = SimControls::new(sender);
    assert!(!controls.send(&snapshot(vec![press("time_pause")], false)));
}
