//! The widget library through the real input path (milestone 8, slice 2): `ck-input`'s `Mapper`, the input tick and the UI stash of
//! the link, the pointer routing, `frame_update.pointer` and the library's dispatcher, in a UI VM held mid-callback by a gate.
//!
//! The witness for the dispatcher's overflow rule. A gesture whose `down` is in the stash and whose real `up` was lost to the ring
//! must not be completed by a *later* `up` in the same delivered packet: the library clicks nothing from an overflow packet.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use ck_input::{Bindings, Mapper, POINTER_CAPACITY};
use ck_lua::{ConfigValue, HostFunction};
use ck_resources::{Assets, Mode, ResourceLimits, ResourceService, ServiceWakes};
use ck_window::{PixelSize, PointerEvent, PointerPosition, ScrollUnit, WindowDesc};

use super::input_tick_tests::{Tables, answer, camera_of, field, tick, update, wait_entered};
use super::*;
use crate::game::controls::SimControls;
use crate::game::slot::SimSlot;
use crate::testing::{Extras, Gate, Harness, frame_extension};
use crate::{font, ui_link::SimLink};

/// The UI script: one button, `A`, at the window's top-left corner; every update dispatches its pointer and records what the
/// library did, then holds on the gate.
const SCRIPT: &str = r#"
local button = require("lib.button")
local pointer = require("lib.pointer")
local clicks = 0
local a = button.new{x = 0, y = 0, text = "A", on_click = function() clicks = clicks + 1 end}
frame.on_update(function(f)
  local p = f.pointer
  local kinds = {}
  if p then
    for _, e in ipairs(p.events) do
      kinds[#kinds + 1] = e.kind .. ":" .. tostring(e.button or e.key)
    end
  end
  pointer.dispatch(p)
  window.record({
    overflow = p ~= nil and p.overflow or false,
    clicks = clicks,
    pressed = a.pressed,
    events = table.concat(kinds, ","),
    key = a.key,
  })
  window.hold()
end)
"#;

pub(super) fn points(x: f64, y: f64) -> PointerPosition {
    PointerPosition::from_points(x, y).expect("a position")
}

fn text_of(table: &ConfigValue, name: &str) -> String {
    match field(table, name) {
        ConfigValue::String(text) => text.clone(),
        other => panic!("{name}: a string, got {other:?}"),
    }
}

fn assert_number(table: &ConfigValue, name: &str, expected: i64) {
    match field(table, name) {
        ConfigValue::Integer(number) => assert_eq!(*number, expected, "{name}"),
        ConfigValue::Float(number) => {
            assert_eq!(number.to_string(), expected.to_string(), "{name}");
        }
        other => panic!("{name}: a number, got {other:?}"),
    }
}

fn flag_of(table: &ConfigValue, name: &str) -> bool {
    match field(table, name) {
        ConfigValue::Bool(flag) => *flag,
        other => panic!("{name}: a boolean, got {other:?}"),
    }
}

fn clicks_of(table: &ConfigValue) -> i64 {
    match field(table, "clicks") {
        ConfigValue::Integer(count) => *count,
        other => panic!("clicks: an integer, got {other:?}"),
    }
}

/// Runs one tick of the real path: the mapper's snapshot goes to the link's input step, and then it is acknowledged, as the loop does.
pub(super) fn run_tick(mapper: &mut Mapper, link: &mut CodekarlUi) {
    let snapshot = mapper.snapshot();
    let sequence = snapshot.sequence;
    link.input(&tick(snapshot)).expect("the tick is consumed");
    mapper.acknowledge(sequence);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one real-input scenario follows move, middle lock, a functional close control and a map press through the held UI VM"
)]
fn real_pointer_ticks_keep_a_locked_tooltip_for_a_functional_close_and_map_down_closes_it() {
    const SCRIPT: &str = r#"
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local button = require("lib.button")
local clicks = 0
local source_x, source_y
local root = button.new{x = 0, y = 0, text = "A", on_click = function() clicks = clicks + 1 end}
local closer = button.new{x = 300, y = 0, text = "Close", on_click = function() clicks = clicks + 1 end}
tooltip.attach(root.key, function(x, y) source_x, source_y = x, y; return {"live tooltip"} end)
frame.on_update(function(f)
  local p = f.pointer
  local kinds = {}
  if p then
    for _, e in ipairs(p.events) do kinds[#kinds + 1] = e.kind .. ":" .. tostring(e.key or e.button) end
  end
  pointer.dispatch(p)
  tooltip.update(f)
  window.record({root = root.key, closer = closer.key, clicks = clicks, events = table.concat(kinds, ","),
    pointer_x = p and p.x, pointer_y = p and p.y, source_x = source_x, source_y = source_y})
  window.hold()
end)
"#;
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .canonicalize()
        .expect("the shipped assets");
    let service = ResourceService::with_assets(
        ResourceLimits::DEFAULT,
        Mode::Backend,
        ServiceWakes {
            jobs: None,
            uploads: None,
            retirements: None,
        },
        Some(Assets::new(assets.clone())),
    )
    .expect("resource service");
    let gate = Arc::new(Mutex::new(Gate::new()));
    let entered = Arc::new(AtomicBool::new(false));
    let hold_next = Arc::new(AtomicBool::new(false));
    let holds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tables: Tables = Arc::default();
    let hold: HostFunction = {
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        let hold_next = Arc::clone(&hold_next);
        let holds = Arc::clone(&holds);
        Arc::new(move |_| {
            holds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if hold_next.swap(false, std::sync::atomic::Ordering::SeqCst) {
                entered.store(true, std::sync::atomic::Ordering::SeqCst);
                Arc::clone(&gate.lock().unwrap_or_else(PoisonError::into_inner)).wait();
            }
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
        SCRIPT,
        Extras {
            functions: vec![("hold", hold), ("record", record)],
            extensions: vec![
                frame_extension(),
                font::extension(Arc::new(font::load_all(&assets).expect("fonts"))),
                crate::theme::extension(Arc::new(crate::theme::load(&assets).expect("theme"))),
                crate::config::TooltipsConfig {
                    lock: "middle_mouse",
                    ..crate::config::TooltipsConfig::default()
                }
                .extension(),
            ],
            assets: Some(assets.clone()),
            service: Some(service),
            modules: Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui"),
            ),
            ..Extras::default()
        },
    );
    let sink = Arc::new(group_tests::Last::default());
    let attached: Arc<dyn ck_render_2d::QuadSink> =
        Arc::clone(&sink) as Arc<dyn ck_render_2d::QuadSink>;
    harness
        .bridge
        .attach(
            &WindowDesc {
                title: "tooltip witness".to_owned(),
                width: 400,
                height: 320,
                clear: [0.0; 3],
                pixel_request: Some(ck_window::PixelRequest {
                    size: (800, 640),
                    min: Some((800, 640)),
                }),
                ..WindowDesc::default()
            },
            PixelSize {
                width: 800,
                height: 640,
            },
            attached,
        )
        .expect("the pixel-open witness window is attached");
    let mut controller = crate::map_app::tests::hover_controller();
    let outside = crate::map_app::tests::hover_pointer(&mut controller);
    let map = crate::map_app::shared(controller);
    let (sender, _receiver) = mpsc::channel();
    let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map))
        .with_scene_bridge(Arc::clone(&harness.bridge))
        .with_sim(SimLink {
            views: Arc::default(),
            controls: SimControls::new(sender),
            slot: SimSlot::new(Arc::new(|| {})),
            time_registered: Arc::new(AtomicBool::new(false)),
            hover_registered: Arc::new(AtomicBool::new(false)),
            select_registered: Arc::new(AtomicBool::new(false)),
        });
    let mut mapper = Mapper::new(Bindings::from_entries(Vec::new()).expect("bindings"));
    run_tick(&mut mapper, &mut link);
    hold_next.store(true, std::sync::atomic::Ordering::SeqCst);
    link.post(update(1)).expect("first update");
    wait_entered(&entered);
    Arc::clone(&gate.lock().unwrap_or_else(PoisonError::into_inner)).open();
    assert_eq!(answer(&mut link).answer, Answer::Returned);

    let root_point = points(4.0, 4.0);
    assert_eq!(
        harness.bridge.hit_regions().unwrap().key_at(root_point),
        Some("button_1")
    );
    mapper.pointer(PointerEvent::Move(root_point));
    for sequence in 2..=22 {
        run_tick(&mut mapper, &mut link);
        link.post(update(sequence)).expect("hover update");
        assert_eq!(answer(&mut link).answer, Answer::Returned);
    }
    assert!(
        tables.lock().unwrap_or_else(PoisonError::into_inner).len() >= 22,
        "each held-gate frame delivered once"
    );
    let saved = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let hover = saved
        .iter()
        .find(|table| match table {
            ConfigValue::Table(fields) => fields.contains_key("source_x"),
            _ => false,
        })
        .expect("the tooltip source received pointer coordinates");
    assert_number(hover, "source_x", 8);
    assert_number(hover, "source_y", 8);
    assert_number(hover, "pointer_x", 8);
    assert_number(hover, "pointer_y", 8);

    // Middle press on the hovered source locks the tooltip; its panel hit region becomes published.
    mapper.pointer(PointerEvent::ButtonDown {
        button: ck_window::PointerButton::Middle,
        position: root_point,
    });
    run_tick(&mut mapper, &mut link);
    link.post(update(23)).expect("middle lock update");
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    mapper.pointer(PointerEvent::ButtonUp {
        button: ck_window::PointerButton::Middle,
        position: root_point,
    });
    run_tick(&mut mapper, &mut link);
    link.post(update(24)).expect("middle release update");
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    let regions = harness.bridge.hit_regions().expect("locked panel hit");
    let panel_point = (0..160)
        .flat_map(|y| (0..240).map(move |x| points(f64::from(x), f64::from(y))))
        .find(|position| {
            regions
                .key_at(*position)
                .is_some_and(|key| key.starts_with("tooltip_panel_"))
        })
        .expect("the locked tooltip has a panel hit");

    // A real button down/up packet is classified before the button callback; it must leave the tooltip locked.
    let close_point = points(155.0, 2.0);
    assert_eq!(
        harness
            .bridge
            .hit_regions()
            .unwrap()
            .key_at(points(310.0, 4.0)),
        Some("button_2")
    );
    mapper.pointer(PointerEvent::Down(close_point));
    mapper.pointer(PointerEvent::Up(close_point));
    run_tick(&mut mapper, &mut link);
    link.post(update(25)).expect("functional close packet");
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    assert!(
        harness
            .bridge
            .hit_regions()
            .unwrap()
            .key_at(panel_point)
            .is_some_and(|key| key.starts_with("tooltip_panel_"))
    );

    // A press outside all widgets reaches the map as exactly one map_down and closes every tooltip.
    let before = tables.lock().unwrap_or_else(PoisonError::into_inner).len();
    let map_point = outside;
    mapper.pointer(PointerEvent::Down(map_point));
    mapper.pointer(PointerEvent::Up(map_point));
    run_tick(&mut mapper, &mut link);
    let map_gate = Gate::new();
    *gate.lock().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&map_gate);
    entered.store(false, std::sync::atomic::Ordering::SeqCst);
    hold_next.store(true, std::sync::atomic::Ordering::SeqCst);
    link.post(update(27)).expect("map press update");
    wait_entered(&entered);
    assert_eq!(
        tables.lock().unwrap_or_else(PoisonError::into_inner).len(),
        before + 1,
        "the press is recorded once while the VM is held"
    );
    map_gate.open();
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    assert!(
        !harness
            .bridge
            .hit_regions()
            .unwrap()
            .key_at(panel_point)
            .is_some_and(|key| key.starts_with("tooltip_panel_")),
        "the map press retires the tooltip panel hit"
    );
    let recorded = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let events = text_of(recorded.last().expect("last update"), "events");
    assert_eq!(events.matches("map_down").count(), 1, "{events}");
    assert!(
        !events.contains("up"),
        "a map-owned up is not routed to the UI: {events}"
    );
    assert_eq!(
        recorded.len(),
        before + 1,
        "the held UI callback delivered once"
    );
    run_tick(&mut mapper, &mut link);
    link.post(update(28))
        .expect("update after the held map press");
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    let after = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let later_events = text_of(after.last().expect("following update"), "events");
    assert!(
        !later_events.contains("map_down"),
        "the following update carries no repeated map_down: {later_events}"
    );
    assert_eq!(holds.load(std::sync::atomic::Ordering::SeqCst), 27);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one two-tick scenario read top to bottom: the tick that retains, the tick that loses, the release and the delivery"
)]
fn a_lost_up_and_a_later_up_in_the_same_delivery_click_nothing_and_the_map_owns_the_final_gesture()
{
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .canonicalize()
        .expect("the shipped assets");
    let service = ResourceService::with_assets(
        ResourceLimits::DEFAULT,
        Mode::Backend,
        ServiceWakes {
            jobs: None,
            uploads: None,
            retirements: None,
        },
        Some(Assets::new(assets.clone())),
    )
    .expect("a resource service");
    let gate = Gate::new();
    let entered = Arc::new(AtomicBool::new(false));
    let tables: Tables = Arc::default();
    let hold: HostFunction = {
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        Arc::new(move |_| {
            entered.store(true, std::sync::atomic::Ordering::SeqCst);
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
        SCRIPT,
        Extras {
            functions: vec![("hold", hold), ("record", record)],
            extensions: vec![
                frame_extension(),
                font::extension(Arc::new(
                    font::load_all(&assets).expect("the shipped fonts"),
                )),
                crate::theme::extension(Arc::new(
                    crate::theme::load(&assets).expect("the shipped theme"),
                )),
            ],
            assets: Some(assets.clone()),
            service: Some(service),
            modules: Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui"),
            ),
            ..Extras::default()
        },
    );
    let mut controller = crate::map_app::tests::hover_controller();
    let outside = crate::map_app::tests::hover_pointer(&mut controller);
    let map = crate::map_app::shared(controller);
    let (sender, _receiver) = mpsc::channel();
    let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map))
        .with_scene_bridge(Arc::clone(&harness.bridge))
        .with_sim(SimLink {
            views: Arc::default(),
            controls: SimControls::new(sender),
            slot: SimSlot::new(Arc::new(|| {})),
            time_registered: Arc::new(AtomicBool::new(false)),
            hover_registered: Arc::new(AtomicBool::new(false)),
            select_registered: Arc::new(AtomicBool::new(false)),
        });
    let regions = harness
        .bridge
        .hit_regions()
        .expect("startup published the button's hit region");
    let inside = points(4.0, 4.0);
    assert_eq!(
        regions.key_at(inside),
        Some("button_1"),
        "the button A is at the corner"
    );
    assert_eq!(
        regions.key_at(outside),
        None,
        "the map's point is outside A"
    );

    let mut mapper = Mapper::new(Bindings::from_entries(Vec::new()).expect("default bindings"));

    // The first update is accepted and its callback holds the UI VM.
    run_tick(&mut mapper, &mut link);
    link.post(update(1)).expect("the first update");
    wait_entered(&entered);

    // Tick 1: `down(A)` is retained and goes into the stash.
    mapper.pointer(PointerEvent::Down(inside));
    run_tick(&mut mapper, &mut link);

    // Tick 2: more than `POINTER_CAPACITY` pointer events, so the ring drops `up(A)` and the second `down(A)`; a later `up(A)` is
    // retained, and then a `down` outside every region.
    mapper.pointer(PointerEvent::Up(inside));
    mapper.pointer(PointerEvent::Down(inside));
    for step in 0..POINTER_CAPACITY - 2 {
        // Distinct positions, so that no two scrolls coalesce.
        let x = 100.0 + f64::from(u16::try_from(step).expect("a small step"));
        mapper.pointer(PointerEvent::Scroll {
            position: points(x, 100.0),
            amount: 256,
            unit: ScrollUnit::Lines,
        });
    }
    mapper.pointer(PointerEvent::Up(inside));
    mapper.pointer(PointerEvent::Down(outside));
    let lost = mapper.snapshot();
    assert!(
        lost.pointer_overflow,
        "the ring lost events: up(A) and the second down(A)"
    );
    assert_eq!(lost.pointer_events.len(), POINTER_CAPACITY);
    assert_eq!(
        &lost.pointer_events[POINTER_CAPACITY - 2..],
        [PointerEvent::Up(inside), PointerEvent::Down(outside)],
        "the retained tail is a later up(A) and the down outside"
    );
    assert!(
        !lost
            .pointer_events
            .iter()
            .any(|event| matches!(event, PointerEvent::Down(position) if *position == inside)),
        "the second down(A) is gone"
    );
    let sequence = lost.sequence;
    link.input(&tick(lost)).expect("the second tick");
    mapper.acknowledge(sequence);

    // The release: the held callback returns and the next accepted update carries the stash.
    gate.open();
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    link.post(update(2)).expect("the second update");
    assert_eq!(answer(&mut link).answer, Answer::Returned);

    let recorded = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(recorded.len(), 2);
    let delivered = &recorded[1];
    assert!(
        flag_of(delivered, "overflow"),
        "the delivered packet carries the overflow"
    );
    let events = text_of(delivered, "events");
    assert!(
        !events.contains("down:button_1") && events.ends_with(",up:button_1,map_down:false"),
        "overflow drops the stale down while the retained up and map press reach the library: {events}"
    );
    assert_eq!(
        clicks_of(delivered),
        0,
        "no click: the library clicks nothing from an overflow packet"
    );
    assert!(
        !flag_of(delivered, "pressed"),
        "A ends not pressed, whatever the order of the events"
    );

    // Non-left buttons travel through the same real tick, stash and Lua dispatcher, but are not left-click widget activation.
    for event in [
        PointerEvent::ButtonDown {
            button: ck_window::PointerButton::Middle,
            position: inside,
        },
        PointerEvent::ButtonUp {
            button: ck_window::PointerButton::Middle,
            position: inside,
        },
        PointerEvent::ButtonDown {
            button: ck_window::PointerButton::Right,
            position: inside,
        },
        PointerEvent::ButtonUp {
            button: ck_window::PointerButton::Right,
            position: inside,
        },
    ] {
        mapper.pointer(event);
    }
    run_tick(&mut mapper, &mut link);
    link.post(update(3)).expect("the non-left button update");
    assert_eq!(answer(&mut link).answer, Answer::Returned);
    let recorded = tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let buttons = &recorded[2];
    assert_eq!(
        text_of(buttons, "events"),
        "button_down:middle,button_up:middle,button_down:right,button_up:right"
    );
    assert_eq!(clicks_of(buttons), 0, "non-left buttons do not click");
    assert!(
        !flag_of(buttons, "pressed"),
        "non-left buttons do not press A"
    );

    // The map owns the final gesture: the button is still held, and moving it drags the map.
    let before = camera_of(&map);
    let moved = PointerPosition {
        x: outside.x + 40 * 256,
        y: outside.y,
    };
    mapper.pointer(PointerEvent::Move(moved));
    run_tick(&mut mapper, &mut link);
    assert_ne!(
        camera_of(&map),
        before,
        "the held gesture belongs to the map: it panned"
    );
}
