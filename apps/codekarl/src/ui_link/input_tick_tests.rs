//! The input tick in the application (milestone 8, slice 0): the map and the time controls take the input at the tick, with the UI VM held
//! mid-callback; the UI's share is the bounded stash, delivered by the next accepted post.
//!
//! A real UI VM is held by a test-only host function that waits on a gate the test opens. Nothing sleeps for a delay: every wait is on an
//! observed state, bounded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_input::{Bindings, Mapper, POINTER_CAPACITY};
use ck_lua::{ConfigValue, HostFunction};
use ck_window::{
    ActionSnapshot, InputSnapshot, InputTick, PixelSize, PointerEvent, PostError, ScrollUnit,
    Transition, UiEvent, UiLink, UpdateEvent, WindowMetrics,
};

use super::*;
use crate::frame_api::FrameRegistration;
use crate::game::controls::{Control, SimControls};
use crate::game::core::Message;
use crate::game::slot::SimSlot;
use crate::testing::{Extras, Gate, Harness, WINDOW, frame_extension};

fn action(name: &str, held: bool, pressed: bool, released: bool) -> ActionSnapshot {
    ActionSnapshot {
        name: name.to_owned(),
        held,
        pressed,
        released,
    }
}

fn edge(name: &str, edge: Edge) -> Transition {
    Transition {
        action: name.to_owned(),
        edge,
    }
}

pub(super) fn metrics() -> WindowMetrics {
    WindowMetrics {
        points: (400, 320),
        pixels: PixelSize {
            width: 800,
            height: 640,
        },
    }
}

/// The action names of the run, all idle.
const NAMES: [&str; 8] = [
    "map_pan_left",
    "map_pan_right",
    "map_zoom_in",
    "map_zoom_out",
    "other",
    "time_pause",
    "x",
    "y",
];

/// A snapshot with every action idle, to be edited.
pub(super) fn idle() -> InputSnapshot {
    InputSnapshot {
        sequence: 1,
        actions: NAMES
            .iter()
            .map(|name| action(name, false, false, false))
            .collect(),
        transitions: Vec::new(),
        overflow: false,
        reset: false,
        overflow_count: 0,
        pointer_events: Vec::new(),
        pointer_position: None,
        pointer_held: false,
        pointer_overflow: false,
    }
}

/// Sets one action of `input`.
fn with(
    mut input: InputSnapshot,
    name: &str,
    held: bool,
    pressed: bool,
    released: bool,
) -> InputSnapshot {
    let entry = input
        .actions
        .iter_mut()
        .find(|entry| entry.name == name)
        .expect("a known action");
    entry.held = held;
    entry.pressed = pressed;
    entry.released = released;
    input
}

pub(super) fn tick(input: InputSnapshot) -> InputTick {
    InputTick {
        dt: Duration::from_millis(16),
        metrics: metrics(),
        input,
    }
}

pub(super) fn update(sequence: u64) -> UiEvent {
    UiEvent::Update(UpdateEvent {
        sequence,
        elapsed: Duration::from_millis(16),
        dt: Duration::from_millis(16),
        metrics: metrics(),
    })
}

/// The next answer, within a bound.
pub(super) fn answer(link: &mut CodekarlUi) -> UiAnswer {
    let give_up = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(answer) = link.take_answer() {
            return answer;
        }
        assert!(Instant::now() < give_up, "no answer in time");
        thread::sleep(Duration::from_millis(1));
    }
}

/// What a `frame_update` callback was given, in order.
pub(super) type Tables = Arc<Mutex<Vec<ConfigValue>>>;

pub(super) fn field<'a>(table: &'a ConfigValue, name: &str) -> &'a ConfigValue {
    let ConfigValue::Table(fields) = table else {
        panic!("a table: {table:?}");
    };
    &fields[name]
}

fn transitions_of(table: &ConfigValue) -> Vec<(String, String)> {
    let items = match field(table, "transitions") {
        ConfigValue::Array(items) => items.clone(),
        // An empty Lua table has no array form once it crosses the VM.
        ConfigValue::Table(fields) if fields.is_empty() => Vec::new(),
        other => panic!("an array: {other:?}"),
    };
    items
        .iter()
        .map(|item| {
            let (ConfigValue::String(name), ConfigValue::String(kind)) =
                (field(item, "action"), field(item, "kind"))
            else {
                panic!("strings");
            };
            (name.clone(), kind.clone())
        })
        .collect()
}

fn flags_of(table: &ConfigValue, name: &str) -> (bool, bool, bool) {
    let actions = field(table, "actions");
    let item = field(actions, name);
    let flag = |key: &str| matches!(field(item, key), ConfigValue::Bool(true));
    (flag("held"), flag("pressed"), flag("released"))
}

fn flag(table: &ConfigValue, name: &str) -> bool {
    matches!(field(table, name), ConfigValue::Bool(true))
}

/// A UI VM whose `frame_update` records its table and then holds on the gate; `entered` says a callback is running.
struct Held {
    harness: Harness,
    gate: Arc<Gate>,
    entered: Arc<AtomicBool>,
    tables: Tables,
}

fn held_ui() -> Held {
    let gate = Gate::new();
    let entered = Arc::new(AtomicBool::new(false));
    let tables: Tables = Arc::default();
    let hold: HostFunction = {
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        Arc::new(move |_| {
            entered.store(true, Ordering::SeqCst);
            gate.wait();
            Ok(Vec::new())
        })
    };
    let record: HostFunction = {
        let tables = Arc::clone(&tables);
        Arc::new(move |arguments| {
            if let Some(Some(value)) = arguments.first() {
                tables
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(value.clone());
            }
            Ok(Vec::new())
        })
    };
    let harness = Harness::new(
        "frame.on_update(function(f) window.record(f) window.hold() end)",
        Extras {
            functions: vec![("hold", hold), ("record", record)],
            extensions: vec![frame_extension()],
            ..Extras::default()
        },
    );
    Held {
        harness,
        gate,
        entered,
        tables,
    }
}

pub(super) fn wait_entered(entered: &AtomicBool) {
    let give_up = Instant::now() + Duration::from_secs(10);
    while !entered.load(Ordering::SeqCst) {
        assert!(Instant::now() < give_up, "the callback did not start");
        thread::yield_now();
    }
}

/// A link with the map controller of the hover fixture and a simulation whose controls the test can read.
fn mapped_link(
    held: &Held,
) -> (
    CodekarlUi,
    crate::map_app::SharedController,
    mpsc::Receiver<Message>,
    PointerPosition,
) {
    let mut controller = crate::map_app::tests::hover_controller();
    let pointer = crate::map_app::tests::hover_pointer(&mut controller);
    let map = crate::map_app::shared(controller);
    let (sender, receiver) = mpsc::channel();
    let link =
        CodekarlUi::with_map(held.harness.ui.slot.clone(), Arc::clone(&map)).with_sim(SimLink {
            views: Arc::default(),
            controls: SimControls::new(sender),
            slot: SimSlot::new(Arc::new(|| {})),
            time_registered: Arc::new(AtomicBool::new(false)),
            hover_registered: Arc::new(AtomicBool::new(false)),
            select_registered: Arc::new(AtomicBool::new(false)),
        });
    (link, map, receiver, pointer)
}

#[test]
fn raw_mapper_loss_with_a_retained_up_is_not_a_map_click_after_routing_to_idle() {
    let held = held_ui();
    let (mut link, map, _receiver, marker) = mapped_link(&held);
    let mut mapper = Mapper::new(Bindings::from_entries(Vec::new()).expect("default bindings"));

    mapper.pointer(PointerEvent::Down(marker));
    let first = mapper.snapshot();
    let first_sequence = first.sequence;
    link.input(&tick(first))
        .expect("map Down routes through CK-207");
    mapper.acknowledge(first_sequence);

    // Alternate move/scroll records so the mapper cannot coalesce adjacent moves. The ring loses
    // earlier details while retaining the final Up at the marker.
    for index in 0..=POINTER_CAPACITY {
        let position = PointerPosition {
            x: marker.x + i32::try_from(index % 2).expect("small offset") * 256,
            y: marker.y,
        };
        mapper.pointer(PointerEvent::Move(position));
        mapper.pointer(PointerEvent::Scroll {
            position,
            amount: 0,
            unit: ScrollUnit::Lines,
        });
    }
    mapper.pointer(PointerEvent::Up(marker));
    let lost = mapper.snapshot();
    assert!(
        lost.pointer_overflow,
        "the real mapper reports raw pointer loss"
    );
    assert_eq!(lost.pointer_events.last(), Some(&PointerEvent::Up(marker)));
    let sequence = lost.sequence;
    link.input(&tick(lost))
        .expect("retained events route through CK-207");
    mapper.acknowledge(sequence);

    assert_eq!(
        *link.state.pointer_owner.borrow(),
        GestureOwner::Idle,
        "the final Up leaves no map owner, so CK-207 masks the resync flag"
    );
    assert_eq!(
        crate::map_app::lock(&map).selection(),
        None,
        "raw loss still reaches MapController separately and disqualifies the gesture"
    );
}

pub(super) fn camera_of(map: &crate::map_app::SharedController) -> Option<ck_map::Camera> {
    crate::map_app::lock(map).camera()
}

#[test]
fn a_published_scene_hit_routes_down_and_up_without_a_map_windowless() {
    let received: Tables = Arc::default();
    let record: HostFunction = {
        let received = Arc::clone(&received);
        Arc::new(move |arguments| {
            if let Some(Some(value)) = arguments.first() {
                received
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(value.clone());
            }
            Ok(Vec::new())
        })
    };
    let held = Harness::new(
        r#"
render.set_hit("fixture_panel", {{x=0, y=0, width=40, height=40}})
frame.on_update(function(update) window.record(update.pointer.events) end)
"#,
        Extras {
            functions: vec![("record", record)],
            extensions: vec![frame_extension()],
            ..Extras::default()
        },
    );
    let regions = held
        .bridge
        .hit_regions()
        .expect("startup completion published the set_hit region with the scene");
    let position = PointerPosition {
        x: 10 * 256,
        y: 12 * 256,
    };
    assert_eq!(regions.key_at(position), Some("fixture_panel"));

    let mut link =
        CodekarlUi::new(held.ui.slot.clone()).with_scene_bridge(Arc::clone(&held.bridge));
    let mut input = idle();
    input.pointer_position = Some(position);
    input.pointer_events = vec![PointerEvent::Down(position), PointerEvent::Up(position)];
    link.input(&tick(input))
        .expect("the pointer tick is consumed");
    link.post(update(1)).expect("the real UI update is posted");
    assert_eq!(answer(&mut link).answer, Answer::Returned);

    let received = received.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(received.len(), 1, "one real UI frame_update callback");
    let ConfigValue::Array(events) = &received[0] else {
        panic!("pointer events array: {:?}", received[0]);
    };
    let observed: Vec<_> = events
        .iter()
        .map(|event| {
            (
                match field(event, "kind") {
                    ConfigValue::String(kind) => kind.as_str(),
                    other => panic!("event kind string: {other:?}"),
                },
                match field(event, "key") {
                    ConfigValue::String(key) => key.as_str(),
                    other => panic!("event key string: {other:?}"),
                },
            )
        })
        .collect();
    assert_eq!(
        observed,
        [("down", "fixture_panel"), ("up", "fixture_panel")]
    );
}

#[test]
fn input_suppresses_map_hover_over_a_published_ui_region_and_restores_it_outside() {
    let mut probe = crate::map_app::tests::hover_controller();
    let pointer = crate::map_app::tests::hover_pointer(&mut probe);
    let left = f64::from(pointer.x) / 256.0;
    let top = f64::from(pointer.y) / 256.0;
    let held = Harness::new(
        &format!(
            "render.set_hit(\"fixture_panel\", {{{{x={left}, y={top}, width=2, height=2}}}})\n"
        ),
        Extras::default(),
    );
    let controller = crate::map_app::tests::hover_controller_with_bridge(Arc::clone(&held.bridge));
    let map = crate::map_app::shared(controller);
    let mut link = CodekarlUi::with_map(held.ui.slot.clone(), Arc::clone(&map))
        .with_scene_bridge(Arc::clone(&held.bridge));

    let mut over_region = idle();
    over_region.pointer_position = Some(pointer);
    over_region.pointer_events = vec![PointerEvent::Move(pointer)];
    link.input(&tick(over_region)).expect("UI hover is routed");
    assert!(
        crate::map_app::lock(&map).hover().is_none(),
        "a published UI region suppresses map hover through input()"
    );

    let mut down = idle();
    down.pointer_position = Some(pointer);
    down.pointer_held = true;
    down.pointer_events = vec![PointerEvent::Down(pointer)];
    link.input(&tick(down)).expect("UI capture starts");
    assert!(crate::map_app::lock(&map).hover().is_none());

    let mut release = idle();
    release.pointer_position = Some(pointer);
    release.pointer_events = vec![PointerEvent::Up(pointer)];
    link.input(&tick(release))
        .expect("UI capture releases over the marker");
    assert_eq!(
        crate::map_app::lock(&map).selection(),
        None,
        "a UI region over a marker owns the whole gesture"
    );

    let outside = PointerPosition {
        x: pointer.x + 3 * 256,
        y: pointer.y,
    };
    let mut leave = idle();
    leave.pointer_position = Some(outside);
    leave.pointer_events = vec![PointerEvent::Up(outside), PointerEvent::Move(outside)];
    link.input(&tick(leave))
        .expect("UI capture ends outside the region");
    assert!(
        crate::map_app::lock(&map).hover().is_some(),
        "map hover resumes after leaving the UI region"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one held-UI scenario, read top to bottom: ticks while held, the release, the next post"
)]
fn the_map_and_the_time_controls_take_the_input_at_each_tick_while_the_ui_is_held_and_the_ui_gets_all_of_it_after()
 {
    let held = held_ui();
    let (mut link, map, receiver, pointer) = mapped_link(&held);
    // The first update is accepted, and its callback holds the UI VM.
    link.input(&tick(idle())).expect("the first tick");
    link.post(update(1)).expect("the first update");
    wait_entered(&held.entered);
    assert_eq!(link.outstanding(), 1);

    // Hover follows the pointer at the tick, and a pause press reaches the simulation once, at its tick.
    let mut hover = with(idle(), "time_pause", false, true, false);
    hover.transitions = vec![edge("time_pause", Edge::Press)];
    hover.pointer_position = Some(pointer);
    hover.pointer_events = vec![PointerEvent::Move(pointer)];
    link.input(&tick(hover)).expect("a hover tick");
    assert!(
        crate::map_app::lock(&map).hover().is_some(),
        "the hover followed the pointer while the UI was held"
    );
    assert_eq!(
        receiver.try_iter().collect::<Vec<_>>(),
        [Message::Control(Control::TogglePause)],
        "the pause press reached the simulation once, at its tick"
    );

    // Held pan keys move the camera at the tick.
    let before = camera_of(&map);
    let mut pan = with(idle(), "map_pan_left", true, true, false);
    pan.transitions = vec![edge("map_pan_left", Edge::Press)];
    link.input(&tick(pan)).expect("a pan tick");
    let panned = camera_of(&map);
    assert_ne!(
        panned, before,
        "the held pan key moved the camera while the UI was held"
    );

    // A pointer drag pans.
    let moved = PointerPosition {
        x: pointer.x + 40 * 256,
        y: pointer.y,
    };
    let mut drag = with(idle(), "map_pan_left", false, false, true);
    drag.transitions = vec![edge("map_pan_left", Edge::Release)];
    drag.pointer_held = true;
    drag.pointer_position = Some(moved);
    drag.pointer_events = vec![PointerEvent::Down(pointer), PointerEvent::Move(moved)];
    link.input(&tick(drag)).expect("a drag tick");
    let dragged = camera_of(&map);
    assert_ne!(
        dragged, panned,
        "the drag moved the camera while the UI was held"
    );

    // A scroll zooms: one line out, anchored at the pointer.
    let mut scroll = idle();
    scroll.pointer_position = Some(moved);
    scroll.pointer_events = vec![PointerEvent::Scroll {
        position: moved,
        amount: -256,
        unit: ScrollUnit::Lines,
    }];
    link.input(&tick(scroll)).expect("a scroll tick");
    let scrolled = camera_of(&map);
    assert_ne!(
        scrolled, dragged,
        "the scroll zoomed the view while the UI was held"
    );

    // A zoom key changes the view (and the tick carries a reset).
    let mut zoom = with(idle(), "map_zoom_in", true, true, false);
    zoom.transitions = vec![edge("map_zoom_in", Edge::Press)];
    zoom.reset = true;
    link.input(&tick(zoom)).expect("a zoom tick");
    assert_ne!(
        camera_of(&map),
        scrolled,
        "the zoom key changed the view while the UI was held"
    );
    assert_eq!(
        link.outstanding(),
        1,
        "the UI VM is still held, with nothing new posted"
    );

    // The release: the held callback returns, and the next accepted update carries everything the ticks stashed, in order.
    held.gate.open();
    let first = answer(&mut link);
    assert_eq!(first.answer, Answer::Returned);
    link.post(update(2)).expect("the second update");
    let second = answer(&mut link);
    assert_eq!(second.answer, Answer::Returned);
    let tables = held
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(tables.len(), 2);
    assert_eq!(
        transitions_of(&tables[1]),
        [
            ("time_pause".to_owned(), "press".to_owned()),
            ("map_pan_left".to_owned(), "press".to_owned()),
            ("map_pan_left".to_owned(), "release".to_owned()),
            ("map_zoom_in".to_owned(), "press".to_owned()),
        ],
        "every stashed transition, in order, once"
    );
    assert_eq!(flags_of(&tables[1], "map_pan_left"), (false, true, true));
    assert_eq!(flags_of(&tables[1], "map_zoom_in"), (true, true, false));
    assert_eq!(flags_of(&tables[1], "time_pause"), (false, true, false));
    assert!(flag(&tables[1], "reset"), "reset is OR-ed");
    assert!(!flag(&tables[1], "overflow"));

    // The next one carries only new input: no transitions, no latches; the held state stays.
    link.input(&tick(with(idle(), "map_zoom_in", true, false, false)))
        .expect("a quiet tick");
    link.post(update(3)).expect("the third update");
    let _ = answer(&mut link);
    let tables = held
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(transitions_of(&tables[2]).is_empty());
    assert_eq!(flags_of(&tables[2], "map_zoom_in"), (true, false, false));
    assert!(!flag(&tables[2], "reset"));
    assert!(
        receiver.try_iter().next().is_none(),
        "the posts acted on nothing: the time controls took their input at the ticks"
    );
}

#[test]
fn three_hundred_transitions_while_held_keep_the_newest_256_and_set_overflow() {
    let held = held_ui();
    let mut link = held.harness.ui.clone();
    link.input(&tick(idle())).expect("the first tick");
    link.post(update(1)).expect("the first update");
    wait_entered(&held.entered);
    for block in 0..3 {
        let mut input = idle();
        input.transitions = (0..100)
            .map(|index| edge(&format!("t{:03}", block * 100 + index), Edge::Press))
            .collect();
        link.input(&tick(input)).expect("a tick");
    }
    held.gate.open();
    let _ = answer(&mut link);
    link.post(update(2)).expect("the second update");
    let _ = answer(&mut link);
    let tables = held
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let kept = transitions_of(&tables[1]);
    assert_eq!(kept.len(), 256);
    assert_eq!(kept[0].0, "t044", "the oldest 44 were dropped");
    assert_eq!(kept[255].0, "t299");
    assert!(flag(&tables[1], "overflow"));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the single scenario verifies refused then accepted posts deliver the same tick exactly once"
)]
fn a_post_refused_as_full_and_then_accepted_delivers_the_stash_once_and_the_controllers_saw_it_once()
 {
    let gate = Gate::new();
    let hold: HostFunction = {
        let gate = Arc::clone(&gate);
        Arc::new(move |_| {
            gate.wait();
            Ok(Vec::new())
        })
    };
    let tables: Tables = Arc::default();
    let record: HostFunction = {
        let tables = Arc::clone(&tables);
        Arc::new(move |arguments| {
            if let Some(Some(value)) = arguments.first() {
                tables
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(value.clone());
            }
            Ok(Vec::new())
        })
    };
    let harness = Harness::new(
        &format!(
            "{WINDOW}\nrender.set_hit(\"panel\", {{{{x=0, y=0, width=40, height=40}}}})\nwindow.on_opened(function() window.hold() end)\nframe.on_update(function(f) window.record(f) end)"
        ),
        Extras {
            functions: vec![("hold", hold), ("record", record)],
            extensions: vec![frame_extension()],
            ..Extras::default()
        },
    );
    let (sender, receiver) = mpsc::channel();
    let mut link = harness
        .ui
        .clone()
        .with_scene_bridge(Arc::clone(&harness.bridge))
        .with_sim(SimLink {
            views: Arc::default(),
            controls: SimControls::new(sender),
            slot: SimSlot::new(Arc::new(|| {})),
            time_registered: Arc::new(AtomicBool::new(false)),
            hover_registered: Arc::new(AtomicBool::new(false)),
            select_registered: Arc::new(AtomicBool::new(false)),
        });
    // Three calls in flight leave only the place reserved for lifecycle posts: an update is refused.
    for _ in 0..3 {
        link.post(UiEvent::Opened).expect("within the limit");
    }
    let mut input = with(idle(), "time_pause", false, true, false);
    input.transitions = vec![edge("time_pause", Edge::Press)];
    let pointer = PointerPosition {
        x: 10 * 256,
        y: 12 * 256,
    };
    input.pointer_position = Some(pointer);
    input.pointer_events = vec![PointerEvent::Down(pointer), PointerEvent::Up(pointer)];
    link.input(&tick(input)).expect("the tick");
    assert_eq!(link.post(update(1)), Err(PostError::Full));
    assert_eq!(
        receiver.try_iter().count(),
        1,
        "the press acted at its tick, though the post was refused"
    );
    gate.open();
    for _ in 0..3 {
        let _ = answer(&mut link);
    }
    link.post(update(2)).expect("accepted");
    let _ = answer(&mut link);
    let tables = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(tables.len(), 1);
    assert_eq!(
        transitions_of(&tables[0]),
        [("time_pause".to_owned(), "press".to_owned())],
        "the stash arrived once, with the accepted post"
    );
    let ConfigValue::Array(pointer_events) = field(field(&tables[0], "pointer"), "events") else {
        panic!("pointer events array: {:?}", field(&tables[0], "pointer"));
    };
    assert_eq!(
        pointer_events.len(),
        2,
        "the pointer pair is delivered once"
    );
    let kinds: Vec<_> = pointer_events
        .iter()
        .map(|event| match field(event, "kind") {
            ConfigValue::String(kind) => kind.as_str(),
            other => panic!("event kind string: {other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["down", "up"]);
    assert!(pointer_events.iter().all(|event| matches!(
        field(event, "key"),
        ConfigValue::String(key) if key == "panel"
    )));
    assert_eq!(
        receiver.try_iter().count(),
        0,
        "the accepted post acted on nothing more"
    );
}

#[test]
fn with_no_callback_registered_the_map_still_pans_by_keys_and_by_drag() {
    // The UI VM registered no `frame_update`; the registration says so (never published), and the stash keeps nothing but held state.
    let held = held_ui();
    let registration = FrameRegistration::default();
    let (link, map, _receiver, pointer) = mapped_link(&held);
    let mut link = link.with_registration(registration);
    let before = camera_of(&map);
    let mut pan = with(idle(), "map_pan_left", true, true, false);
    pan.transitions = vec![edge("map_pan_left", Edge::Press)];
    link.input(&tick(pan)).expect("a pan tick");
    let panned = camera_of(&map);
    assert_ne!(panned, before, "keys pan without a UI callback");
    let mut drag = idle();
    drag.pointer_held = true;
    drag.pointer_position = Some(PointerPosition {
        x: pointer.x + 40 * 256,
        y: pointer.y,
    });
    drag.pointer_events = vec![
        PointerEvent::Down(pointer),
        PointerEvent::Move(PointerPosition {
            x: pointer.x + 40 * 256,
            y: pointer.y,
        }),
    ];
    link.input(&tick(drag)).expect("a drag tick");
    assert_ne!(camera_of(&map), panned, "a drag pans without a UI callback");
    // Nothing of the events is stashed for a UI that has no callback; the held state is.
    let stash = link.state.stash.borrow();
    assert_eq!(stash.transitions().count(), 0);
    assert!(!stash.overflow() && !stash.reset());
}

/// A link over a VM that records every table, with the registration the test publishes by hand (it plays the setter's observer).
fn registered_link() -> (CodekarlUi, FrameRegistration, Tables, Harness) {
    let tables: Tables = Arc::default();
    let record: HostFunction = {
        let tables = Arc::clone(&tables);
        Arc::new(move |arguments| {
            if let Some(Some(value)) = arguments.first() {
                tables
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(value.clone());
            }
            Ok(Vec::new())
        })
    };
    let harness = Harness::new(
        "frame.on_update(function(f) window.record(f) end)",
        Extras {
            functions: vec![("record", record)],
            extensions: vec![frame_extension()],
            ..Extras::default()
        },
    );
    let registration = FrameRegistration::default();
    let link = harness.ui.clone().with_registration(registration.clone());
    (link, registration, tables, harness)
}

fn press_of(name: &str) -> InputSnapshot {
    let mut input = with(idle(), name, true, true, false);
    input.transitions = vec![edge(name, Edge::Press)];
    input
}

fn delivered(link: &mut CodekarlUi, tables: &Tables, sequence: u64) -> ConfigValue {
    link.post(update(sequence)).expect("accepted");
    let _ = answer(link);
    tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .last()
        .cloned()
        .expect("a table")
}

#[test]
fn registering_later_delivers_nothing_from_before_the_first_tick_that_saw_the_new_generation() {
    let (mut link, registration, tables, _harness) = registered_link();
    // Not registered yet: a press is not kept.
    link.input(&tick(press_of("x"))).expect("a tick");
    // The observer publishes; the next tick is the boundary. A press at that tick is delivered; the one before never is.
    registration.publish(true);
    link.input(&tick(press_of("y"))).expect("the boundary tick");
    let table = delivered(&mut link, &tables, 1);
    assert_eq!(
        transitions_of(&table),
        [("y".to_owned(), "press".to_owned())],
        "from the boundary tick on, nothing before it"
    );
}

#[test]
fn a_register_and_unregister_within_one_tick_leave_the_stash_trimmed() {
    let (mut link, registration, tables, _harness) = registered_link();
    registration.publish(true);
    link.input(&tick(press_of("x"))).expect("a registered tick");
    // Between two ticks the script registers and unregisters: one generation change, the final bit.
    registration.publish(true);
    registration.publish(false);
    link.input(&tick(press_of("y"))).expect("the next tick");
    // The stash holds nothing of either press; only the held state follows.
    let table = delivered(&mut link, &tables, 1);
    assert!(transitions_of(&table).is_empty());
    assert_eq!(flags_of(&table, "y"), (true, false, false));
}

#[test]
fn a_not_registered_answer_for_an_earlier_epoch_neither_clears_the_new_stash_nor_follows_it() {
    // A VM with no callback: the update posted earlier is answered `NotRegistered`, and that answer arrives after the registration.
    let harness = Harness::new(
        "",
        Extras {
            extensions: vec![frame_extension()],
            ..Extras::default()
        },
    );
    let registration = FrameRegistration::default();
    let mut link = harness.ui.clone().with_registration(registration.clone());
    link.input(&tick(idle())).expect("a tick");
    link.post(update(1)).expect("posted");
    registration.publish(true);
    link.input(&tick(press_of("x"))).expect("the boundary tick");
    let stale = answer(&mut link);
    assert_eq!(
        stale.answer,
        Answer::NotRegistered,
        "the earlier update found no callback"
    );
    let stash = link.state.stash.borrow();
    assert_eq!(
        stash
            .transitions()
            .map(|t| t.action.as_str())
            .collect::<Vec<_>>(),
        ["x"],
        "the answer left the new registration's stash alone (and it is not mapped to a success)"
    );
}

#[test]
fn the_controlled_interleaving_delivers_the_press_that_the_boundary_tick_stashed() {
    let (mut link, registration, tables, _harness) = registered_link();
    // 1. a tick while unregistered
    link.input(&tick(idle())).expect("step 1");
    // 2. one publish of registered = true
    registration.publish(true);
    // 3. a tick with a press: it observes the new generation, clears, then stashes the press
    link.input(&tick(press_of("x"))).expect("step 3");
    // 4. a tick with no new publish and no new input
    link.input(&tick(with(idle(), "x", true, false, false)))
        .expect("step 4");
    let table = delivered(&mut link, &tables, 1);
    assert_eq!(
        transitions_of(&table),
        [("x".to_owned(), "press".to_owned())],
        "the press is delivered by the next accepted post and was not cleared at step 4"
    );
    assert_eq!(flags_of(&table, "x"), (true, true, false));
}

/// The reviewer's ordering case, through the real setter: an old press is stashed, the script re-registers between two ticks (the
/// observer publishes the new generation and advances the control's epoch, which restarts the UI cursor at once), and the update
/// is posted before the next tick. The old press must not reach the new callback; the first press from a tick that follows must.
#[test]
fn a_re_registration_between_ticks_never_lets_an_older_press_reach_the_new_callback_in_a_post_before_the_next_tick()
 {
    use crate::frame_api::{FrameBackend, extension};
    use ck_window::UpdateSettings;

    let tables: Tables = Arc::default();
    let record: HostFunction = {
        let tables = Arc::clone(&tables);
        Arc::new(move |arguments| {
            if let Some(Some(value)) = arguments.first() {
                tables
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(value.clone());
            }
            Ok(Vec::new())
        })
    };
    let control = UpdateSettings::new(60, Duration::from_millis(100))
        .expect("valid pacing")
        .control();
    let registration = FrameRegistration::default();
    let setter = extension(
        true,
        FrameBackend::Control(control.clone()),
        registration.clone(),
    );
    let harness = Harness::new(
        &format!(
            "{WINDOW}\nframe.on_update(function(f) window.record(f) end)\n\
             window.on_opened(function() frame.on_update(function(f) window.record(f) end) end)"
        ),
        Extras {
            functions: vec![("record", record)],
            extensions: vec![setter],
            ..Extras::default()
        },
    );
    let mut link = harness.ui.clone().with_registration(registration.clone());
    let (generation_before, registered) = registration.state();
    assert!(registered, "the script registered at load");

    // 1. A tick stashes an old press under the first registration.
    link.input(&tick(press_of("x"))).expect("a tick");
    // 2. The script registers again (the real setter and its observer): a new generation, and `UpdateControl::set` advances the
    // control's epoch, which restarts the UI cursor (so the update below is due at once).
    link.post(UiEvent::Opened).expect("opened is posted");
    // (`opened` returns nothing, which the link maps to `Decline`; the loop ignores it for `opened`.)
    assert_eq!(answer(&mut link).answer, Answer::Decline);
    assert_ne!(
        registration.state().0,
        generation_before,
        "a new generation"
    );
    // 3. The restarted UI cursor posts before the next tick has run.
    link.post(update(1)).expect("the update is posted");
    let _ = answer(&mut link);
    let first = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .last()
        .cloned()
        .expect("a table");
    assert!(
        transitions_of(&first).is_empty(),
        "the older registration's press did not reach the new callback: {first:?}"
    );
    assert_eq!(
        flags_of(&first, "x"),
        (true, false, false),
        "only its held state remains"
    );
    // 4. From the next tick on, input is delivered.
    link.input(&tick(press_of("y"))).expect("the next tick");
    let second = delivered(&mut link, &tables, 2);
    assert_eq!(
        transitions_of(&second),
        [("y".to_owned(), "press".to_owned())]
    );
}

/// One tick in which `name` is held, pressed and in the transitions, as a key press delivers it.
fn key_down(name: &str) -> InputSnapshot {
    let mut input = idle();
    input.actions = vec![action(name, true, true, false)];
    input.transitions = vec![edge(name, Edge::Press)];
    input
}

/// What one game key does to the map and to the simulation, on a fresh link, with the pause menu's key block `blocked`: whether the
/// camera moved, and the controls the simulation was sent.
fn effect_of_key(name: &str, blocked: bool) -> (bool, Vec<Message>) {
    let held = held_ui();
    let (mut link, map, receiver, _pointer) = mapped_link(&held);
    link.sim
        .as_ref()
        .expect("a simulation")
        .controls
        .set_keys_blocked(blocked);
    let before = camera_of(&map);
    link.input(&tick(key_down(name))).expect("a key tick");
    let stashed: Vec<_> = link
        .state
        .stash
        .borrow()
        .transitions()
        .map(|transition| (transition.action.clone(), transition.edge))
        .collect();
    assert_eq!(
        stashed,
        [(name.to_owned(), Edge::Press)],
        "the UI is given the key either way: the block is on the game's actions, not on the UI's input"
    );
    (camera_of(&map) != before, receiver.try_iter().collect())
}

#[test]
fn the_key_block_stops_each_game_key_at_the_map_and_the_simulation_and_leaves_the_rest() {
    for name in crate::game::controls::BLOCKED_ACTIONS {
        let (moved, controls) = effect_of_key(name, false);
        assert!(
            moved || !controls.is_empty(),
            "{name}: with no block the key acts, or this test proves nothing"
        );
        let (moved, controls) = effect_of_key(name, true);
        assert!(!moved, "{name}: the camera stays");
        assert!(
            controls.is_empty(),
            "{name}: no control reaches the simulation: {controls:?}"
        );
    }
    // Escape and any other action are not the game's keys: they still reach the UI (checked in `effect_of_key`) and are not touched.
    for name in ["close", "other"] {
        let (moved, controls) = effect_of_key(name, true);
        assert!(
            !moved && controls.is_empty(),
            "{name} has no game effect to block"
        );
    }
}

#[test]
fn a_held_map_key_stops_while_the_block_is_on_and_a_press_in_the_tick_before_it_still_acts() {
    let held = held_ui();
    let (mut link, map, receiver, _pointer) = mapped_link(&held);
    let controls = link.sim.as_ref().expect("a simulation").controls.clone();
    // The tick of the opening Escape was taken before the UI turned the block on: its key acts.
    let start = camera_of(&map);
    link.input(&tick(key_down("map_pan_left"))).expect("tick");
    let first = camera_of(&map);
    assert_ne!(first, start, "same-tick keys may act");
    controls.set_keys_blocked(true);
    // The key is still held: every tick it would pan, and none does.
    let mut held_key = with(idle(), "map_pan_left", true, false, false);
    held_key.transitions = Vec::new();
    for _ in 0..3 {
        link.input(&tick(held_key.clone())).expect("tick");
    }
    assert_eq!(
        camera_of(&map),
        first,
        "a held pan key does nothing under the block"
    );
    controls.set_keys_blocked(false);
    link.input(&tick(held_key)).expect("tick");
    assert_ne!(
        camera_of(&map),
        first,
        "and acts again when the block is off"
    );
    assert!(receiver.try_iter().next().is_none());
}

#[test]
fn the_tick_the_block_turns_on_ends_a_map_drag_already_captured() {
    let held = held_ui();
    let (mut link, map, _receiver, pointer) = mapped_link(&held);
    let controls = link.sim.as_ref().expect("a simulation").controls.clone();
    let at = |dx: i32| PointerPosition {
        x: pointer.x + dx * 256,
        y: pointer.y,
    };
    let start = camera_of(&map);
    let mut press = idle();
    press.pointer_held = true;
    press.pointer_position = Some(at(0));
    press.pointer_events = vec![PointerEvent::Down(at(0))];
    link.input(&tick(press)).expect("the press");
    let mut drag = |dx: i32| {
        let mut input = idle();
        input.pointer_held = true;
        input.pointer_position = Some(at(dx));
        input.pointer_events = vec![PointerEvent::Move(at(dx))];
        link.input(&tick(input)).expect("a drag tick");
        camera_of(&map)
    };
    let panned = drag(30);
    // The drag pans while nothing blocks it.
    assert_ne!(panned, start);
    controls.set_keys_blocked(true);
    assert_eq!(
        drag(60),
        panned,
        "the drag ends with the tick the block turns on"
    );
    assert_eq!(
        drag(90),
        panned,
        "and the held button's later moves do nothing"
    );
    controls.set_keys_blocked(false);
    assert_eq!(drag(120), panned, "the drag does not come back by itself");
}

#[test]
fn a_second_opening_ends_a_drag_captured_after_the_first_closed() {
    let held = held_ui();
    let (mut link, map, _receiver, pointer) = mapped_link(&held);
    let controls = link.sim.as_ref().expect("a simulation").controls.clone();
    let at = |dx: i32| PointerPosition {
        x: pointer.x + dx * 256,
        y: pointer.y,
    };
    let drag_tick = |link: &mut CodekarlUi, events: Vec<PointerEvent>, held: bool, dx: i32| {
        let mut input = idle();
        input.pointer_held = held;
        input.pointer_position = Some(at(dx));
        input.pointer_events = events;
        link.input(&tick(input)).expect("a pointer tick");
        camera_of(&map)
    };
    // The menu opens and closes once with no gesture in flight.
    controls.set_keys_blocked(true);
    drag_tick(&mut link, Vec::new(), false, 0);
    controls.set_keys_blocked(false);
    drag_tick(&mut link, Vec::new(), false, 0);
    // A new drag is captured by the map, and the menu opens again: this opening ends it too.
    drag_tick(&mut link, vec![PointerEvent::Down(at(0))], true, 0);
    let panned = drag_tick(&mut link, vec![PointerEvent::Move(at(30))], true, 30);
    controls.set_keys_blocked(true);
    assert_eq!(
        drag_tick(&mut link, vec![PointerEvent::Move(at(60))], true, 60),
        panned,
        "the second opening ends the drag the map had captured"
    );
    assert_eq!(
        drag_tick(&mut link, vec![PointerEvent::Move(at(90))], true, 90),
        panned
    );
}
