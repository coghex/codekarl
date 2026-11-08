//! Scene groups through the real input path and the widget library (milestone 8, slice 4): routing against a group's offset, clip and
//! visibility, the `inside` each routed event carries, a button's click decided from it, the library's `group` option, and the
//! placement of a button's backgrounds at the edges of the point domain.
//!
//! A real UI VM runs the library over the real `render` API and a real scene bridge with a recording sink; ticks come from `ck-input`'s
//! `Mapper`. Nothing sleeps: every update, and every posted callback, is awaited.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use ck_input::{Bindings, Mapper};
use ck_lua::{ConfigValue, HostFunction};
use ck_render_2d::{FrameQuad, Metrics, POINT_DOMAIN, QuadSink, Snapshot};
use ck_resources::{Assets, Mode, ResourceLimits, ResourceService, ServiceWakes};
use ck_window::{PixelSize, PointerEvent, WindowDesc};

use super::input_tick_tests::{Tables, answer, field, update};
use super::widget_witness_tests::{points, run_tick};
use super::*;
use crate::game::controls::SimControls;
use crate::game::slot::SimSlot;
use crate::testing::{Extras, Harness, frame_extension};
use crate::{font, ui_link::SimLink};

/// What the sink last saw.
#[derive(Default)]
struct Last(Mutex<Option<Snapshot>>);

impl QuadSink for Last {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(snapshot);
        None
    }
}

impl Last {
    fn snapshot(&self) -> Snapshot {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .expect("a published snapshot")
    }

    fn quads(&self) -> Vec<FrameQuad> {
        self.snapshot().scene.expect("a frame").quads().to_vec()
    }
}

/// The script every rig runs after the test's own: it dispatches each update's pointer and records what the library did.
const PRELUDE: &str = r#"
local pointer = require("lib.pointer")
clicks = 0
function click() clicks = clicks + 1 end
"#;

const REPORT: &str = r#"
frame.on_update(function(f)
  local p = f.pointer
  local kinds = {}
  if p then
    for _, e in ipairs(p.events) do
      kinds[#kinds + 1] = e.kind .. ":" .. tostring(e.key) .. ":" .. tostring(e.inside)
    end
  end
  pointer.dispatch(p)
  window.record({
    clicks = clicks,
    events = table.concat(kinds, ","),
    pressed = a ~= nil and a.pressed or false,
    look = a ~= nil and a.look or "",
  })
end)
"#;

struct Rig {
    harness: Harness,
    link: CodekarlUi,
    mapper: Mapper,
    sink: Arc<Last>,
    tables: Tables,
    sequence: u64,
}

impl Rig {
    /// A UI VM whose script is the prelude, `script` and the reporting callback. The window is 400 by 320 points at 2 pixels per point.
    fn new(script: &str) -> Self {
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
            &format!("{PRELUDE}\n{script}\n{REPORT}"),
            Extras {
                functions: vec![("record", record)],
                extensions: vec![
                    frame_extension(),
                    font::extension(Arc::new(font::load(&assets).expect("the shipped font"))),
                ],
                assets: Some(assets),
                service: Some(service),
                modules: Some(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui"),
                ),
                ..Extras::default()
            },
        );
        let sink = Arc::new(Last::default());
        let attached: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
        harness
            .bridge
            .attach(
                &WindowDesc {
                    title: "t".to_owned(),
                    width: 400,
                    height: 320,
                    clear: [0.0; 3],
                },
                PixelSize {
                    width: 800,
                    height: 640,
                },
                attached,
            )
            .expect("attached");
        let controller = crate::map_app::tests::hover_controller();
        let map = crate::map_app::shared(controller);
        let (sender, _receiver) = mpsc::channel();
        let link = CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map))
            .with_scene_bridge(Arc::clone(&harness.bridge))
            .with_sim(SimLink {
                controls: SimControls::new(sender),
                slot: SimSlot::new(Arc::new(|| {})),
                time_registered: Arc::new(AtomicBool::new(false)),
                hover_registered: Arc::new(AtomicBool::new(false)),
            });
        Self {
            harness,
            link,
            mapper: Mapper::new(Bindings::from_entries(Vec::new()).expect("default bindings")),
            sink,
            tables,
            sequence: 0,
        }
    }

    /// One input tick with these pointer events.
    fn feed(&mut self, events: &[PointerEvent]) {
        for event in events {
            self.mapper.pointer(*event);
        }
        run_tick(&mut self.mapper, &mut self.link);
    }

    /// Posts the next `frame_update`, waits for it, and returns what the script recorded for it.
    fn deliver(&mut self) -> ConfigValue {
        self.sequence += 1;
        self.link
            .post(update(self.sequence))
            .expect("the update is accepted");
        assert_eq!(answer(&mut self.link).answer, Answer::Returned);
        self.tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .cloned()
            .expect("the update recorded a summary")
    }

    /// Runs the script's `window.on_opened` callback and waits (its edits are published by the completion hook).
    fn opened(&mut self) {
        self.link.post(UiEvent::Opened).expect("posted");
        let answered = answer(&mut self.link).answer;
        // Only a failure matters for `Opened`.
        assert!(
            !matches!(answered, Answer::Failed { .. }),
            "the opened callback failed: {answered:?}"
        );
    }

    /// The topmost published key at a point, if any.
    fn key_at(&self, x: f64, y: f64) -> Option<String> {
        self.harness
            .bridge
            .hit_regions()
            .expect("published regions")
            .key_at(points(x, y))
            .map(str::to_owned)
    }
}

fn clicks(summary: &ConfigValue) -> i64 {
    match field(summary, "clicks") {
        ConfigValue::Integer(count) => *count,
        other => panic!("clicks: an integer, got {other:?}"),
    }
}

fn text(summary: &ConfigValue, name: &str) -> String {
    match field(summary, name) {
        ConfigValue::String(text) => text.clone(),
        other => panic!("{name}: a string, got {other:?}"),
    }
}

fn pressed(summary: &ConfigValue) -> bool {
    matches!(field(summary, "pressed"), ConfigValue::Bool(true))
}

fn down(x: f64, y: f64) -> PointerEvent {
    PointerEvent::Down(points(x, y))
}

fn up(x: f64, y: f64) -> PointerEvent {
    PointerEvent::Up(points(x, y))
}

/// A button "Go" at local (10, 10) in a group offset by (100, 50): its published rectangle is x 110..142, y 60..90.
const OFFSET_BUTTON: &str = r#"
local button = require("lib.button")
g = render.create_group()
render.set_group_offset(g, 100, 50)
a = button.new{x = 10, y = 10, text = "Go", on_click = click, group = g}
window.on_opened(function() end)
"#;

// ---------------------------------------------------------------------------------------------------------------------
// Routing with groups, and the `inside` of each event
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_press_and_release_at_a_grouped_buttons_translated_position_clicks_once() {
    let mut rig = Rig::new(OFFSET_BUTTON);
    assert_eq!(rig.key_at(115.0, 65.0).as_deref(), Some("button_1"));
    rig.feed(&[down(115.0, 65.0)]);
    rig.feed(&[up(115.0, 65.0)]);
    let summary = rig.deliver();
    assert_eq!(
        text(&summary, "events"),
        "down:button_1:true,up:button_1:true"
    );
    assert_eq!(clicks(&summary), 1);
    assert!(!pressed(&summary));
}

#[test]
fn a_press_at_the_untranslated_position_goes_to_the_map() {
    let mut rig = Rig::new(OFFSET_BUTTON);
    // The button was laid out at local (10, 10); without its group's offset that is where a naive hit test would look.
    assert_eq!(rig.key_at(15.0, 15.0), None);
    rig.feed(&[down(15.0, 15.0)]);
    rig.feed(&[up(15.0, 15.0)]);
    let summary = rig.deliver();
    assert_eq!(text(&summary, "events"), "", "the UI was given nothing");
    assert_eq!(clicks(&summary), 0);
}

const CLIPPED_BUTTON: &str = r#"
local button = require("lib.button")
g = render.create_group()
render.set_group_offset(g, 100, 50)
a = button.new{x = 10, y = 10, text = "Go", on_click = click, group = g}
-- Only the left half of the button, x 110..126, is visible.
render.set_group_clip(g, {x = 110, y = 60, width = 16, height = 30})
window.on_opened(function() end)
"#;

#[test]
fn a_release_on_the_visible_half_of_a_clipped_button_clicks() {
    let mut rig = Rig::new(CLIPPED_BUTTON);
    rig.feed(&[down(115.0, 65.0)]);
    rig.feed(&[up(120.0, 65.0)]);
    let summary = rig.deliver();
    assert_eq!(
        text(&summary, "events"),
        "down:button_1:true,up:button_1:true"
    );
    assert_eq!(clicks(&summary), 1);
}

#[test]
fn a_release_on_the_clipped_half_has_inside_false_and_does_not_click() {
    let mut rig = Rig::new(CLIPPED_BUTTON);
    rig.feed(&[down(115.0, 65.0)]);
    // x 135 is inside the button's rectangle and outside its group's clip, so outside the published region.
    rig.feed(&[up(135.0, 65.0)]);
    let summary = rig.deliver();
    assert_eq!(
        text(&summary, "events"),
        "down:button_1:true,up:button_1:false"
    );
    assert_eq!(clicks(&summary), 0);
    assert!(!pressed(&summary), "the gesture ended without a click");
}

#[test]
fn a_press_on_the_clipped_half_is_not_a_hit() {
    let mut rig = Rig::new(CLIPPED_BUTTON);
    assert_eq!(rig.key_at(135.0, 65.0), None);
    rig.feed(&[down(135.0, 65.0)]);
    rig.feed(&[up(135.0, 65.0)]);
    let summary = rig.deliver();
    assert_eq!(text(&summary, "events"), "");
    assert_eq!(clicks(&summary), 0);
}

#[test]
fn a_key_scrolled_outside_its_clip_is_not_hit() {
    let script = r#"
local button = require("lib.button")
g = render.create_group()
-- The clip stays where it is: x 100..150, y 50..100. The button, at x 110..142, starts inside it.
render.set_group_clip(g, {x = 100, y = 50, width = 50, height = 50})
a = button.new{x = 110, y = 60, text = "Go", on_click = click, group = g}
window.on_opened(function() render.set_group_offset(g, 200, 0) end)
"#;
    let mut rig = Rig::new(script);
    assert_eq!(rig.key_at(115.0, 65.0).as_deref(), Some("button_1"));
    // Scrolled away: the button's rectangle is now x 310..342, wholly outside the clip, so nothing is published for it.
    rig.opened();
    assert_eq!(rig.key_at(315.0, 65.0), None);
    assert_eq!(rig.key_at(115.0, 65.0), None);
    assert!(
        rig.harness
            .bridge
            .hit_regions()
            .unwrap()
            .regions()
            .is_empty()
    );
    rig.feed(&[down(315.0, 65.0)]);
    rig.feed(&[up(315.0, 65.0)]);
    let summary = rig.deliver();
    assert_eq!(text(&summary, "events"), "");
    assert_eq!(clicks(&summary), 0);
}

#[test]
fn the_click_follows_the_inside_computed_at_routing_not_the_frame_the_ui_later_publishes() {
    let script = format!(
        "{OFFSET_BUTTON}\nwindow.on_opened(function() render.set_group_offset(g, 300, 250) end)\n"
    );
    let mut rig = Rig::new(&script);
    rig.feed(&[down(115.0, 65.0)]);
    let summary = rig.deliver();
    assert!(pressed(&summary));
    // The up is routed against the regions the player was pointing at: inside.
    rig.feed(&[up(115.0, 65.0)]);
    // Before the UI dispatches it, the UI moves the group and a new offset is published: the button is elsewhere now.
    rig.opened();
    assert_eq!(rig.key_at(115.0, 65.0), None);
    assert_eq!(rig.key_at(315.0, 265.0).as_deref(), Some("button_1"));
    let summary = rig.deliver();
    assert_eq!(text(&summary, "events"), "up:button_1:true");
    assert_eq!(
        clicks(&summary),
        1,
        "the click is the one for the frame the player pointed at"
    );
}

#[test]
fn hiding_the_group_of_a_captured_key_cancels_the_gesture() {
    let script = format!(
        "{OFFSET_BUTTON}\nwindow.on_opened(function() render.set_group_visible(g, false) end)\n"
    );
    let mut rig = Rig::new(&script);
    rig.feed(&[down(115.0, 65.0)]);
    let summary = rig.deliver();
    assert!(pressed(&summary));
    rig.opened();
    assert_eq!(
        rig.key_at(115.0, 65.0),
        None,
        "a hidden group publishes nothing"
    );
    rig.feed(&[PointerEvent::Move(points(116.0, 66.0))]);
    rig.feed(&[up(116.0, 66.0)]);
    let summary = rig.deliver();
    let events = text(&summary, "events");
    assert!(events.contains("cancel:button_1:false"), "{events}");
    assert_eq!(clicks(&summary), 0);
    assert!(!pressed(&summary));
}

// ---------------------------------------------------------------------------------------------------------------------
// The library's group option
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_librarys_group_option_puts_quads_and_keys_in_the_group() {
    let script = r#"
local button = require("lib.button")
local panel = require("lib.panel")
local label = require("lib.label")
g = render.create_group()
render.set_group_offset(g, 100, 50)
loose = panel.new{x = 4, y = 6, width = 10, height = 10}
inside = panel.new{x = 4, y = 6, width = 10, height = 10, group = g}
label.new{x = 20, y = 20, text = "A", group = g}
a = button.new{x = 10, y = 10, text = "Go", on_click = click, group = g}
window.on_opened(function() end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
    let frame = rig.sink.quads();
    let xs: Vec<(f64, f64)> = frame.iter().map(|quad| (quad.x, quad.y)).collect();
    // Painter order: the base group first (the loose panel), then the group in its own creation order: the panel, the label's glyph and
    // the button's three backgrounds and two glyphs. Every grouped quad is translated by the offset; the loose one is not.
    assert_eq!(xs[0], (4.0, 6.0), "the quad without a group");
    assert_eq!(xs[1], (104.0, 56.0), "the grouped panel");
    assert_eq!(xs[2], (120.0, 70.0), "the grouped label's glyph");
    assert_eq!(xs[3], (110.0, 60.0), "the button's normal background");
    assert_eq!(frame.len(), 1 + 1 + 1 + 3 + 2);
    // The hit key is the group's: its region is translated.
    assert_eq!(rig.key_at(115.0, 65.0).as_deref(), Some("button_1"));
    assert_eq!(rig.key_at(15.0, 15.0), None);
    // Destroying the button removes its key (without naming the group).
    let _ = rig.deliver();
}

// ---------------------------------------------------------------------------------------------------------------------
// Parking at the bounds of the point domain
// ---------------------------------------------------------------------------------------------------------------------

/// The window of the rig in pixels, for the independent placement.
const PIXELS: (i64, i64) = (800, 640);

/// Where a rectangle (in points, at 2 pixels per point) shows, computed from first principles: each edge is the rounded product of
/// its coordinate and the scale (halves away from zero), then it is cut to the window and to the clip.
fn expected_destination(
    rect: (f64, f64, f64, f64),
    clip: Option<(f64, f64, f64, f64)>,
) -> Option<(i64, i64, i64, i64)> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "edges in this test are far inside i64"
    )]
    let edge = |value: f64| (value * 2.0).round() as i64;
    let (mut left, mut top) = (edge(rect.0).max(0), edge(rect.1).max(0));
    let (mut right, mut bottom) = (
        edge(rect.0 + rect.2).min(PIXELS.0),
        edge(rect.1 + rect.3).min(PIXELS.1),
    );
    if let Some(clip) = clip {
        left = left.max(edge(clip.0));
        top = top.max(edge(clip.1));
        right = right.min(edge(clip.0 + clip.2));
        bottom = bottom.min(edge(clip.1 + clip.3));
    }
    (left < right && top < bottom).then_some((left, top, right, bottom))
}

fn metrics() -> Metrics {
    Metrics {
        points_width: 400,
        points_height: 320,
        pixels: PixelSize {
            width: 800,
            height: 640,
        },
    }
}

fn placed(quad: &FrameQuad) -> Option<(i64, i64, i64, i64)> {
    quad.place(&metrics()).map(|placed| {
        let d = placed.destination;
        (
            i64::from(d.left),
            i64::from(d.top),
            i64::from(d.right),
            i64::from(d.bottom),
        )
    })
}

/// A button "Go" (32 by 30 points) at `local` in a group offset by `offset`, in the state `state`; returns the three backgrounds'
/// frame quads and the current one's index.
fn backgrounds(offset: (f64, f64), local: (f64, f64), state: &str) -> (Vec<FrameQuad>, usize) {
    let drive = match state {
        "normal" => "",
        "hover" => "pointer.dispatch({over = a.key, overflow = false, held = false, events = {}})",
        "pressed" => {
            "pointer.dispatch({over = a.key, overflow = false, held = true, events = {{kind = 'down', key = a.key, x = 1, y = 1, inside = true}}})"
        }
        other => panic!("{other}"),
    };
    let script = format!(
        r#"
local button = require("lib.button")
g = render.create_group()
render.set_group_offset(g, {}, {})
a = button.new{{x = {}, y = {}, text = "Go", on_click = click, group = g}}
window.on_opened(function() {drive} end)
"#,
        offset.0, offset.1, local.0, local.1
    );
    let mut rig = Rig::new(&script);
    rig.opened();
    let current = match state {
        "normal" => 0,
        "hover" => 1,
        _ => 2,
    };
    (rig.sink.quads()[..3].to_vec(), current)
}

#[test]
fn parked_backgrounds_are_never_placed_and_the_current_one_is_placed_by_the_ordinary_rule_at_the_domains_bounds()
 {
    let edge = POINT_DOMAIN;
    // The button is laid out so that its hit rectangle stays inside the domain under the group's offset.
    let cases = [
        ((edge, edge), (-60.0, -60.0)),
        ((-edge, -edge), (60.0, 60.0)),
        ((0.0, 0.0), (10.0, 10.0)),
    ];
    for (offset, local) in cases {
        for state in ["normal", "hover", "pressed"] {
            let (quads, current) = backgrounds(offset, local, state);
            assert_eq!(quads.len(), 3);
            for (index, quad) in quads.iter().enumerate() {
                let result = placed(quad);
                if index == current {
                    let translated = (offset.0 + local.0, offset.1 + local.1, 32.0, 30.0);
                    assert_eq!(
                        result,
                        expected_destination(translated, None),
                        "{offset:?} {state}: the current background by the ordinary rule"
                    );
                } else {
                    assert_eq!(
                        result, None,
                        "{offset:?} {state}: background {index} is parked and never drawn"
                    );
                }
            }
        }
    }
}

#[test]
fn at_a_normal_position_exactly_one_background_shows_in_each_state() {
    for state in ["normal", "hover", "pressed"] {
        let (quads, current) = backgrounds((100.0, 50.0), (20.0, 10.0), state);
        let shown: Vec<usize> = quads
            .iter()
            .enumerate()
            .filter(|(_, quad)| placed(quad).is_some())
            .map(|(index, _)| index)
            .collect();
        assert_eq!(shown, [current], "{state}");
        assert_eq!(
            placed(&quads[current]),
            expected_destination((120.0, 60.0, 32.0, 30.0), None),
            "{state}"
        );
    }
}

#[test]
fn the_current_background_follows_a_clip_and_a_parked_one_never_does() {
    let script = r#"
local button = require("lib.button")
g = render.create_group()
render.set_group_offset(g, 100, 50)
a = button.new{x = 10, y = 10, text = "Go", on_click = click, group = g}
render.set_group_clip(g, {x = 110, y = 60, width = 16, height = 30})
window.on_opened(function() pointer.dispatch({over = a.key, overflow = false, held = false, events = {}}) end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
    let quads = rig.sink.quads();
    let clip = Some((110.0, 60.0, 16.0, 30.0));
    assert_eq!(placed(&quads[0]), None);
    assert_eq!(placed(&quads[2]), None);
    assert_eq!(
        placed(&quads[1]),
        expected_destination((110.0, 60.0, 32.0, 30.0), clip)
    );
    assert_eq!(placed(&quads[1]), Some((220, 120, 252, 180)));
}
