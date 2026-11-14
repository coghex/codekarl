//! Scene groups through the real input path and the widget library (milestone 8, slice 4): routing against a group's offset, clip and
//! visibility, the `inside` each routed event carries, a button's click decided from it, the library's `group` option, and the
//! placement of a button's backgrounds at the edges of the point domain.
//!
//! A real UI VM runs the library over the real `render` API and a real scene bridge with a recording sink; ticks come from `ck-input`'s
//! `Mapper`. Nothing sleeps: every update, and every posted callback, is awaited.

use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use ck_input::{Bindings, Mapper};
use ck_lua::{ConfigValue, Extension, HostFunction};
use ck_render_2d::{FrameQuad, HitRect, Metrics, POINT_DOMAIN, QuadSink, Snapshot};
use ck_resources::{Assets, Mode, ResourceLimits, ResourceService, ServiceWakes};
use ck_window::{
    CloseSource, PixelSize, PointerEvent, PointerPosition, ScrollUnit, UiEvent, UpdateEvent,
    WindowDesc, WindowMetrics,
};

use super::input_tick_tests::{Tables, answer, field, update};
use super::widget_witness_tests::{points, run_tick};
use super::*;
use crate::game::controls::SimControls;
use crate::game::slot::{PauseReason, SimSlot, SimSnapshot};
use crate::testing::{Extras, Harness, frame_extension};
use crate::{font, ui_link::SimLink};

#[path = "../../tests/support/map_selection_body.rs"]
mod map_selection_fixture;

/// What the sink last saw, and how many snapshots it has seen.
#[derive(Default)]
pub(super) struct Last {
    last: Mutex<Option<Snapshot>>,
    published: std::sync::atomic::AtomicUsize,
}

impl QuadSink for Last {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(snapshot);
        self.published
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        None
    }
}

impl Last {
    /// How many snapshots were published: a callback that makes no scene call publishes none.
    pub(super) fn count(&self) -> usize {
        self.published.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .expect("a published snapshot")
    }

    pub(super) fn quads(&self) -> Vec<FrameQuad> {
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

pub(super) struct Rig {
    pub(super) harness: Harness,
    pub(super) link: CodekarlUi,
    pub(super) map: crate::map_app::SharedController,
    select_registered: Arc<AtomicBool>,
    pub(super) mapper: Mapper,
    pub(super) sink: Arc<Last>,
    pub(super) tables: Tables,
    pub(super) map_pointer: PointerPosition,
    simulation_slot: Arc<SimSlot>,
    /// The run's controls (a clone shares the key block) and the receiving end of the simulation's control channel.
    pub(super) controls: SimControls,
    pub(super) control_receiver: mpsc::Receiver<crate::game::core::Message>,
    sequence: u64,
}

impl Rig {
    /// A UI VM whose script is the prelude, `script` and the reporting callback. The window is 400 by 320 points at 2 pixels per point.
    pub(super) fn new(script: &str) -> Self {
        Self::with_script(
            &format!("{PRELUDE}\n{script}\n{REPORT}"),
            false,
            Vec::new(),
            None,
        )
    }

    /// Runs a fixture's full script without replacing its own frame callback with the reporting callback.
    pub(super) fn new_fixture_body(script: &str) -> Self {
        Self::with_script(script, false, Vec::new(), None)
    }

    /// Runs a fixture body with a replacement `settings` host table.
    pub(super) fn new_fixture_body_with_settings(script: &str, settings: Extension) -> Self {
        Self::with_script(script, false, Vec::new(), Some(settings))
    }

    /// Runs a fixture body with additional host functions, for tests that need the same serialized data as a production API.
    pub(super) fn new_fixture_body_with_functions(
        script: &str,
        functions: Vec<(&'static str, HostFunction)>,
    ) -> Self {
        Self::with_script(script, false, functions, None)
    }

    /// [`new_fixture_body`](Self::new_fixture_body) for a window opened in screen pixels: its UI is in UI units at the scale its drawable
    /// allows.
    pub(super) fn new_fixture_body_in_pixels(script: &str) -> Self {
        Self::with_script(script, true, Vec::new(), None)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the shared real-VM harness setup stays together for all widget and fixture bodies"
    )]
    fn with_script(
        script: &str,
        in_pixels: bool,
        mut extra_functions: Vec<(&'static str, HostFunction)>,
        settings_extension: Option<Extension>,
    ) -> Self {
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
        let time_registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let select_registered = Arc::new(AtomicBool::new(false));
        // One control channel and one key block for the `game` extension and the link, as in a run, so a test sees what the script
        // sent and the link sees the block the script turned on.
        let (control_sender, control_receiver) = mpsc::channel();
        let controls = SimControls::new(control_sender);
        let game = crate::game_api::extension_with_select(
            Arc::clone(&time_registered),
            Arc::clone(&hover_registered),
            Arc::clone(&select_registered),
            Arc::new(|| {}),
            controls.clone(),
        );
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
        extra_functions.insert(0, ("record", record));
        let harness = Harness::new(
            script,
            Extras {
                functions: extra_functions,
                extensions: vec![
                    frame_extension(),
                    game,
                    font::extension(Arc::new(
                        font::load_all(&assets).expect("the shipped fonts"),
                    )),
                    crate::theme::extension(Arc::new(
                        crate::theme::load(&assets).expect("the shipped theme"),
                    )),
                    settings_extension
                        .unwrap_or_else(|| crate::config::TooltipsConfig::default().extension()),
                    crate::glossary::extension(Arc::new(
                        crate::glossary::load(
                            &assets,
                            &font::load_all(&assets).expect("the shipped fonts"),
                        )
                        .expect("the shipped glossary"),
                    )),
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
                    pixel_request: in_pixels.then_some(ck_window::PixelRequest {
                        size: (1280, 800),
                        min: Some((1280, 800)),
                    }),
                    ..WindowDesc::default()
                },
                PixelSize {
                    width: 800,
                    height: 640,
                },
                attached,
            )
            .expect("attached");
        let mut controller = crate::map_app::tests::hover_controller();
        let map_pointer = crate::map_app::tests::hover_pointer(&mut controller);
        let map = crate::map_app::shared(controller);
        let simulation_slot = SimSlot::new(Arc::new(|| {}));
        let link = CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map))
            .with_scene_bridge(Arc::clone(&harness.bridge))
            .with_sim(SimLink {
                views: Arc::default(),
                controls: controls.clone(),
                slot: Arc::clone(&simulation_slot),
                time_registered,
                hover_registered,
                select_registered: Arc::clone(&select_registered),
            });
        Self {
            harness,
            link,
            map,
            select_registered,
            mapper: Mapper::new(Bindings::from_entries(Vec::new()).expect("default bindings")),
            sink,
            tables,
            map_pointer,
            simulation_slot,
            controls,
            control_receiver,
            sequence: 0,
        }
    }

    /// One input tick with these pointer events.
    pub(super) fn feed(&mut self, events: &[PointerEvent]) {
        for event in events {
            self.mapper.pointer(*event);
        }
        run_tick(&mut self.mapper, &mut self.link);
    }

    /// Posts the next `frame_update`, waits for it, and returns what the script recorded for it.
    pub(super) fn deliver(&mut self) -> ConfigValue {
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

    /// Posts a frame update to the fixture's own callback and waits for its completion.
    pub(super) fn deliver_fixture_frame(&mut self) {
        self.sequence += 1;
        self.link
            .post(update(self.sequence))
            .expect("the fixture update is accepted");
        let answered = answer(&mut self.link).answer;
        assert_eq!(
            answered,
            Answer::Returned,
            "fixture frame callback: {answered:?}"
        );
    }

    pub(super) fn deliver_game_time(&mut self, snapshot: &SimSnapshot) {
        self.simulation_slot.publish(snapshot.clone());
        self.deliver_fixture_frame();
        // `take_answer` posts game_time after the frame_update; its answer can still be pending when that frame returns.
        // A following frame is queued behind game_time, so waiting for its answer makes this helper a completion barrier.
        self.deliver_fixture_frame();
    }

    /// Posts a fixture frame at an explicit window size and pixel scale.
    pub(super) fn deliver_fixture_frame_at(&mut self, width: u32, height: u32, scale: u32) {
        self.sequence += 1;
        self.link
            .post(UiEvent::Update(UpdateEvent {
                sequence: self.sequence,
                elapsed: std::time::Duration::from_millis(16),
                dt: std::time::Duration::from_millis(16),
                metrics: WindowMetrics {
                    points: (width, height),
                    pixels: PixelSize {
                        width: width * scale,
                        height: height * scale,
                    },
                },
            }))
            .expect("fixture update is accepted");
        let answered = answer(&mut self.link).answer;
        assert_eq!(
            answered,
            Answer::Returned,
            "fixture frame callback: {answered:?}"
        );
    }

    /// Runs the script's `window.on_opened` callback and waits (its edits are published by the completion hook).
    pub(super) fn opened(&mut self) {
        self.link.post(UiEvent::Opened).expect("posted");
        let answered = answer(&mut self.link).answer;
        // Only a failure matters for `Opened`.
        assert!(
            !matches!(answered, Answer::Failed { .. }),
            "the opened callback failed: {answered:?}"
        );
    }

    /// Routes Escape through the app's real window close-request callback.
    pub(super) fn escape(&mut self) -> bool {
        self.link
            .post(UiEvent::CloseRequested {
                source: CloseSource::Key,
            })
            .expect("the Escape close request is accepted");
        matches!(answer(&mut self.link).answer, Answer::Decline)
    }

    /// The topmost published key at a point, if any.
    pub(super) fn key_at(&self, x: f64, y: f64) -> Option<String> {
        self.harness
            .bridge
            .hit_regions()
            .expect("published regions")
            .key_at(points(x, y))
            .map(str::to_owned)
    }
}

#[test]
fn both_backend_map_selection_bodies_start_and_receive_pointer_events_in_a_real_ui_vm() {
    const CHILD: &str = "CK_UI_LINK_MAP_SELECT_VM_CHILD";
    const WINDOWED: &str = "CK_WINDOWED_TEST_MAP_SELECT";
    const VULKAN: &str = "CK_VULKAN_TEST_MAP_SELECT";
    const TEST_NAME: &str = "ui_link::group_tests::both_backend_map_selection_bodies_start_and_receive_pointer_events_in_a_real_ui_vm";

    if std::env::var_os(CHILD).is_none() {
        assert_both_backend_fixture_vms(CHILD, WINDOWED, VULKAN, TEST_NAME);
        return;
    }

    let markers = std::env::var_os(WINDOWED)
        .or_else(|| std::env::var_os(VULKAN))
        .map(std::path::PathBuf::from)
        .expect("the child has a backend worker marker directory");
    run_map_selection_fixture_vm(&markers);
}

fn assert_both_backend_fixture_vms(child: &str, windowed: &str, vulkan: &str, test_name: &str) {
    for (selected, other) in [(windowed, vulkan), (vulkan, windowed)] {
        let markers = crate::testing::Dir::new();
        let output = Command::new(std::env::current_exe().expect("the current test binary"))
            .args(["--exact", test_name, "--nocapture"])
            .env(child, "1")
            .env(selected, &markers.0)
            .env_remove(other)
            .output()
            .expect("the isolated real-VM fixture test");
        assert!(
            output.status.success(),
            "the real-VM fixture test with {selected} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_fixture_vm_markers(&markers.0, selected);
    }
}

fn assert_fixture_vm_markers(markers: &std::path::Path, backend: &str) {
    for (marker, message) in [
        (
            "frame-ready",
            "fixture body must observe its first frame before the driver acts",
        ),
        (
            "scroll-frame-observed",
            "fixture body must observe a frame after the map-owned scroll",
        ),
        (
            "place-ready",
            "Lua must write place-ready to the worker marker directory",
        ),
        (
            "fixture-frame-pointer-events",
            "fixture body must receive pointer events in frame_update",
        ),
        (
            "help-ready",
            "pointer events must reach the fixture button callback",
        ),
    ] {
        assert!(markers.join(marker).is_file(), "the {backend} {message}");
    }
}

fn run_map_selection_fixture_vm(markers: &std::path::Path) {
    let mut rig = Rig::new_fixture_body(map_selection_fixture::body());
    open_map_selection_fixture(&mut rig, markers);
    verify_map_owned_scroll(&mut rig, markers);
    select_fixture_place(&mut rig, markers);
    verify_map_marker_drag(&mut rig, markers);
    open_and_raise_fixture_windows(&mut rig, markers);
    move_fixture_place_window(&mut rig, markers);
    close_fixture_windows(&mut rig, markers);
}

#[test]
fn real_map_hover_in_main_lua_resolves_the_shared_location_class() {
    let text_field = |table: &ConfigValue, name: &str| match field(table, name) {
        ConfigValue::String(value) => value.clone(),
        other => panic!("{name}: expected a string, got {other:?}"),
    };
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua"),
    )
    .expect("the shipped UI entry point");
    let fixture = r#"
local classes = require("lib.classes")
classes.set_revision(0)
game.places = function()
  return {{id = "fixture-town", name = "Fixture Town", kind = "town", size = 1}}
end
game.place = function(id, revision)
  if id ~= "fixture-town" or revision ~= 0 then return nil end
  return {id = id, workers = 412, dependents = 0, pops = {},
    land = {usable = {}, land = {}, sea = 0},
    last_month = {closed = true, produced = {}, used_by_pops = {}, lost = {}}}
end
local tooltip = require("lib.tooltip")
local map_hover = tooltip.map_hover
tooltip.map_hover = function(hover, source)
  if hover ~= nil then
    local content = tooltip.content(source.class, source.args)
    local spans = {}
    for _, span in ipairs(content[1]) do
      spans[#spans + 1] = type(span) == "table" and span.text or span
    end
    window.record({id = hover.id, class = source.class, title = content.title,
      population = table.concat(spans)})
  end
  map_hover(hover, source)
end
"#;
    let mut rig = Rig::new_fixture_body(&format!("{main}\n{fixture}"));
    rig.opened();
    rig.feed(&[PointerEvent::Move(rig.map_pointer)]);
    for _ in 0..24 {
        rig.deliver_fixture_frame();
    }

    let observed = rig
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|table| text_field(table, "id") == "fixture-town")
        .cloned()
        .expect("the real map controller delivered its marker hover to main.lua");
    assert_eq!(text_field(&observed, "class"), "location");
    assert_eq!(text_field(&observed, "title"), "Fixture Town");
    assert_eq!(text_field(&observed, "population"), "population 412");
}

#[test]
fn the_real_map_population_tooltip_locks_and_opens_its_nested_tooltip() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua"),
    )
    .expect("the shipped UI entry point");
    let fixture = r#"
local classes = require("lib.classes")
classes.set_revision(0)
game.places = function()
  return {{id = "fixture-town", name = "Fixture Town", kind = "town", size = 1}}
end
game.place = function(id, revision)
  if id ~= "fixture-town" or revision ~= 0 then return nil end
  return {id = id, workers = 412, dependents = 0, pops = {},
    land = {usable = {}, land = {}, sea = 0},
    last_month = {closed = true, produced = {}, used_by_pops = {}, lost = {}}}
end
local tooltip = require("lib.tooltip")
local content = tooltip.content
tooltip.content = function(class, args)
  window.record({tooltip_class = class})
  return content(class, args)
end
"#;
    let mut rig = Rig::new_fixture_body(&format!("{main}\n{fixture}"));
    rig.opened();
    rig.feed(&[PointerEvent::Move(rig.map_pointer)]);

    // Timer locking takes the open delay plus the configured one-second lock. This also drives the real render extension,
    // rather than a test double, through the span hit keys that crashed in the owner's run.
    for _ in 0..100 {
        rig.deliver_fixture_frame();
    }
    let hits = rig
        .harness
        .bridge
        .hit_regions()
        .expect("published hit regions");
    let span_keys: Vec<_> = hits
        .regions()
        .iter()
        .filter(|region| region.key().starts_with("tooltip_span_"))
        .map(|region| region.key().to_owned())
        .collect();
    assert!(
        span_keys.len() >= 2,
        "the locked location tooltip publishes its glossary and population spans: {span_keys:?}"
    );

    // The first visible line is `population` plus its linked number. The second span is the number's population link.
    let population_number = span_keys[1].clone();
    let target = pointer_inside_hit_key(&rig, &population_number);
    rig.feed(&[PointerEvent::Move(target)]);
    for _ in 0..100 {
        rig.deliver_fixture_frame();
    }

    let classes: Vec<_> = rig
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(|table| match field(table, "tooltip_class") {
            ConfigValue::String(class) => Some(class.clone()),
            _ => None,
        })
        .collect();
    assert!(
        classes.contains(&"population".to_owned()),
        "the real population span opens its nested tooltip: {classes:?}"
    );
    let panel_keys: Vec<_> = rig
        .harness
        .bridge
        .hit_regions()
        .expect("published hit regions")
        .regions()
        .iter()
        .filter(|region| region.key().starts_with("tooltip_panel_"))
        .map(|region| region.key().to_owned())
        .collect();
    assert!(
        panel_keys.len() >= 2,
        "the root and nested tooltip panels coexist: {panel_keys:?}"
    );

    let published = rig.sink.count();
    for _ in 0..10 {
        rig.deliver_fixture_frame();
    }
    assert_eq!(
        rig.sink.count(),
        published,
        "ten unchanged updates with the locked nested tooltip open publish no snapshots"
    );

    // Match the loop's teardown boundary: the bridge is detached while the resource service and
    // writer slot still exist, then an already-posted update runs with the locked tooltip open.
    // The accepted tree refuses tooltip.update's raise_groups here and reports the close as failed.
    rig.harness.bridge.detach();
    rig.deliver_fixture_frame();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one real-input witness follows label opening, overlap routing, and the underlying speed-control click"
)]
fn a_speed_label_has_no_hit_and_the_real_hit_route_delivers_the_underlying_press() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua"),
    )
    .expect("the shipped UI entry point");
    let fixture = r#"
local panel = require("lib.panel")
local panel_new = panel.new
panel.new = function(spec)
  local value = panel_new(spec)
  if spec.kind == "tooltip" then
    window.record({tooltip_panel = true, x = value.x, y = value.y,
      width = value.width, height = value.height})
  end
  return value
end
local control = game.control
game.control = function(name)
  window.record({control = name})
  return control(name)
end
"#;
    let mut rig = Rig::new_fixture_body(&format!("{fixture}\n{main}"));
    rig.opened();
    rig.deliver_game_time(&crate::readout_tests::snapshot(0, 1, None));
    for _ in 0..4 {
        rig.deliver_fixture_frame();
    }

    let speed_one = (0..400)
        .flat_map(|y| (0..400).map(move |x| (x, y)))
        .find(|(x, y)| rig.key_at(f64::from(*x), f64::from(*y)).as_deref() == Some("speed_1"))
        .expect("the real time bar publishes speed 1's control region");
    let speed_one_point = points(f64::from(speed_one.0) + 0.25, f64::from(speed_one.1) + 0.25);
    rig.feed(&[PointerEvent::Move(speed_one_point)]);
    for _ in 0..40 {
        rig.deliver_fixture_frame();
    }
    let label = rig
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|table| match table {
            ConfigValue::Table(fields) => {
                fields.get("tooltip_panel") == Some(&ConfigValue::Bool(true))
            }
            _ => false,
        })
        .cloned()
        .expect("the speed label's panel is visible in the real scene");
    let number = |name: &str| match field(&label, name) {
        ConfigValue::Integer(value) => {
            f64::from(i32::try_from(*value).expect("fixture UI coordinates fit i32"))
        }
        ConfigValue::Float(value) => *value,
        other => panic!("{name} is numeric: {other:?}"),
    };
    let (left, top, right, bottom) = (
        number("x"),
        number("y"),
        number("x") + number("width"),
        number("y") + number("height"),
    );
    let overlap = (0..400)
        .flat_map(|y| (0..400).map(move |x| (x, y)))
        .map(|(x, y)| (f64::from(x) + 0.25, f64::from(y) + 0.25))
        .find(|(x, y)| {
            *x >= left
                && *x < right
                && *y >= top
                && *y < bottom
                && rig.key_at(*x, *y).as_deref() == Some("speed_2")
        })
        .expect("the label rectangle overlaps speed 2's real control hit region");
    let hit_regions = rig.harness.bridge.hit_regions().expect("real hit regions");
    assert!(
        !hit_regions
            .regions()
            .iter()
            .any(|region| region.key().starts_with("tooltip_panel_")),
        "a label publishes no panel hit key"
    );

    // Move out of speed 1 and click at a point covered by its visible label and speed 2.
    // The real hit router, not this fixture, chooses the widget for the press.
    let click_point = points(overlap.0, overlap.1);
    rig.feed(&[PointerEvent::Move(click_point)]);
    rig.deliver_fixture_frame();
    rig.feed(&[
        PointerEvent::Down(click_point),
        PointerEvent::Up(click_point),
    ]);
    rig.deliver_fixture_frame();
    let controls: Vec<_> = rig
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(|table| match table {
            ConfigValue::Table(fields) => match fields.get("control") {
                Some(ConfigValue::String(name)) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(controls, ["time_speed_2"]);
}

#[test]
fn a_speed_label_draws_its_one_line_in_the_tooltip_text_colour_with_no_number_colour() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua"),
    )
    .expect("the shipped UI entry point");
    let fixture = r#"
local text = require("lib.text")
local theme = require("lib.theme")
local set = text.set
text.set = function(line, value)
  set(line, value)
  if line.font_name == "small" then
    window.record({run = value, colour = line.colour, tooltip_text = theme.kind("tooltip").text,
      number = theme.colour("number"), over_limit = theme.colour("over_limit")})
  end
end
"#;
    let mut rig = Rig::new_fixture_body(&format!("{fixture}\n{main}"));
    rig.opened();
    rig.deliver_game_time(&crate::readout_tests::snapshot(0, 1, None));
    for _ in 0..4 {
        rig.deliver_fixture_frame();
    }
    let speed_three = (0..400)
        .flat_map(|y| (0..400).map(move |x| (x, y)))
        .find(|(x, y)| rig.key_at(f64::from(*x), f64::from(*y)).as_deref() == Some("speed_3"))
        .expect("the real time bar publishes speed 3's control region");
    let before = rig
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len();
    rig.feed(&[PointerEvent::Move(points(
        f64::from(speed_three.0) + 0.25,
        f64::from(speed_three.1) + 0.25,
    ))]);
    for _ in 0..40 {
        rig.deliver_fixture_frame();
    }
    let tables = rig.tables.lock().unwrap_or_else(PoisonError::into_inner);
    let text_of = |table: &ConfigValue, name: &str| match field(table, name) {
        ConfigValue::String(value) => value.clone(),
        other => panic!("{name} is a string: {other:?}"),
    };
    let drawn: Vec<&ConfigValue> = tables[before..]
        .iter()
        .filter(|table| matches!(table, ConfigValue::Table(fields) if fields.contains_key("run")))
        .collect();
    assert!(
        drawn
            .iter()
            .any(|table| text_of(table, "run").contains("speed 3")),
        "the label's one line is drawn whole: {drawn:?}"
    );
    for table in drawn {
        // The colours are read from the theme in the same VM, not copied here.
        assert_ne!(
            text_of(table, "colour"),
            text_of(table, "number"),
            "no label run is in the number colour: {table:?}"
        );
        assert_ne!(
            text_of(table, "colour"),
            text_of(table, "over_limit"),
            "no label run is in the over-limit colour: {table:?}"
        );
        assert_eq!(
            text_of(table, "colour"),
            text_of(table, "tooltip_text"),
            "every label run is in the tooltip text colour: {table:?}"
        );
    }
}

#[test]
fn the_open_pause_label_reads_play_after_a_game_time_update() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua"),
    )
    .expect("the shipped UI entry point");
    let fixture = r#"
local tooltip = require("lib.tooltip")
local label = tooltip.label
tooltip.label = function(key, words)
  if words == "pause" or words == "play" then
    window.record({pause_key = key, label_words = words})
  end
  return label(key, words)
end
"#;
    let mut rig = Rig::new_fixture_body(&format!("{fixture}\n{main}"));
    rig.opened();
    rig.deliver_game_time(&crate::readout_tests::snapshot(0, 1, None));
    for _ in 0..3 {
        rig.deliver_fixture_frame();
    }
    let pause_key = rig
        .tables
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find_map(|table| match table {
            ConfigValue::Table(fields) => fields.get("pause_key").and_then(|value| match value {
                ConfigValue::String(key) => Some(key.clone()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "the pause button exposes its key to this observer: {:?}",
                rig.tables.lock().unwrap_or_else(PoisonError::into_inner)
            )
        });
    let pause_point = points(50.25, 20.25);
    assert_eq!(
        rig.key_at(50.25, 20.25).as_deref(),
        Some(pause_key.as_str()),
        "the time bar publishes the pause button's hit region"
    );
    rig.feed(&[PointerEvent::Move(pause_point)]);
    for _ in 0..40 {
        rig.deliver_fixture_frame();
    }
    rig.deliver_game_time(&crate::readout_tests::snapshot(
        0,
        1,
        Some(PauseReason::Load),
    ));
    assert!(
        rig.tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|table| matches!(table, ConfigValue::Table(fields)
            if fields.get("label_words") == Some(&ConfigValue::String("play".to_owned())))),
        "the real game_time callback updates the pause label to play"
    );
}

fn pointer_inside_hit_key(rig: &Rig, key: &str) -> PointerPosition {
    let hits = rig
        .harness
        .bridge
        .hit_regions()
        .expect("published hit regions");
    for y in (0..320 * 256).step_by(256) {
        for x in (0..400 * 256).step_by(256) {
            let point = PointerPosition { x, y };
            if hits
                .regions()
                .iter()
                .any(|region| region.key() == key && region.rect().contains(point))
            {
                return point;
            }
        }
    }
    panic!("hit key {key} has no point in the fixture window");
}

fn open_map_selection_fixture(rig: &mut Rig, markers: &std::path::Path) {
    rig.opened();
    assert!(
        rig.select_registered
            .load(std::sync::atomic::Ordering::SeqCst),
        "the fixture body registered its game_select callback"
    );
    rig.deliver_fixture_frame();
    assert!(
        markers.join("frame-ready").is_file(),
        "the exact map-selection body must observe its first frame before input"
    );
}

fn verify_map_owned_scroll(rig: &mut Rig, markers: &std::path::Path) {
    rig.feed(&[PointerEvent::Scroll {
        position: rig.map_pointer,
        amount: -256,
        unit: ScrollUnit::Lines,
    }]);
    std::fs::write(markers.join("scroll-injected"), "x")
        .expect("the timer poll marks its map-owned scroll for the next UI frame");
    rig.deliver_fixture_frame();
    assert!(
        markers.join("scroll-frame-observed").is_file(),
        "the exact map-selection body must observe a frame after the scroll stage"
    );
    assert!(
        !markers.join("fixture-frame-pointer-events").is_file(),
        "a map-owned scroll is not delivered as UI pointer events"
    );
}

fn select_fixture_place(rig: &mut Rig, markers: &std::path::Path) {
    // A map-owned click exercises game.on_select and runs the fixture's place-window callback in this UI VM.
    let map_pointer = (
        f64::from(rig.map_pointer.x) / 256.0,
        f64::from(rig.map_pointer.y) / 256.0,
    );
    rig.feed(&[down(map_pointer.0, map_pointer.1)]);
    rig.feed(&[up(map_pointer.0, map_pointer.1)]);
    assert!(
        crate::map_app::lock(&rig.map).selection().is_some(),
        "the fixture click must select its map marker before the link can deliver game_select"
    );
    rig.deliver_fixture_frame();
    // game_select uses its own latest-wins channel, so the first frame's answer intake posts it;
    // the next frame lets the real UI VM run the selection callback before the pointer witness.
    rig.deliver_fixture_frame();
    assert!(
        markers.join("frame-ready").is_file(),
        "the exact map-selection body must run a frame callback before accepting input"
    );
    assert!(
        markers.join("place-ready").is_file(),
        "a retained map click must deliver game_select to the fixture body"
    );
}

fn verify_map_marker_drag(rig: &mut Rig, markers: &std::path::Path) {
    // The window driver observes that dragging a map marker opened nothing before opening Help.
    let dragged = PointerPosition {
        x: rig.map_pointer.x + 20 * 256,
        y: rig.map_pointer.y + 10 * 256,
    };
    rig.feed(&[down(
        f64::from(rig.map_pointer.x) / 256.0,
        f64::from(rig.map_pointer.y) / 256.0,
    )]);
    rig.feed(&[PointerEvent::Move(dragged)]);
    rig.feed(&[up(
        f64::from(dragged.x) / 256.0,
        f64::from(dragged.y) / 256.0,
    )]);
    std::fs::write(markers.join("map-marker-drag-injected"), "x")
        .expect("the map drag stage reached the UI frame callback");
    rig.deliver_fixture_frame();
    assert!(
        markers.join("drag-verified").is_file(),
        "the exact body must verify a map drag leaves the selected place window alone"
    );
}

fn open_and_raise_fixture_windows(rig: &mut Rig, markers: &std::path::Path) {
    // A separate UI-owned click reaches the same body's own frame callback with pointer events.
    rig.feed(&[down(20.0, 20.0), up(20.0, 20.0)]);
    rig.deliver_fixture_frame();
    assert!(
        markers.join("fixture-frame-pointer-events").is_file(),
        "the exact map-selection body must run a pointer-bearing frame_update"
    );
    assert!(
        markers.join("help-ready").is_file(),
        "the pointer events must reach the fixture's actual button callback"
    );
    std::fs::write(markers.join("place-click-injected"), "x")
        .expect("the place-window click stage reached the UI frame callback");
    rig.feed(&[down(50.0, 100.0), up(50.0, 100.0)]);
    rig.deliver_fixture_frame();
    assert!(
        markers.join("place-raised").is_file(),
        "the exact body must observe the place window raised above Help"
    );
}

fn move_fixture_place_window(rig: &mut Rig, markers: &std::path::Path) {
    std::fs::write(markers.join("title-drag-injected"), "x")
        .expect("the title drag stage reached the UI frame callback");
    rig.feed(&[down(20.0, 70.0)]);
    rig.feed(&[PointerEvent::Move(points(50.0, 90.0))]);
    rig.feed(&[up(50.0, 90.0)]);
    rig.deliver_fixture_frame();
    assert!(
        markers.join("title-moved").is_file(),
        "the exact body must observe the place window moved by the title drag"
    );
}

fn close_fixture_windows(rig: &mut Rig, markers: &std::path::Path) {
    for (marker, wanted) in [
        ("place-closed", Answer::Decline),
        ("help-closed", Answer::Decline),
        ("close-requested", Answer::Accept),
    ] {
        rig.link
            .post(UiEvent::CloseRequested {
                source: CloseSource::Key,
            })
            .expect("the Escape stage posts a key close request");
        let got = answer(&mut rig.link);
        assert_eq!(
            got.answer, wanted,
            "the Escape stage returns the right action"
        );
        if marker != "close-requested" {
            assert!(
                !markers.join(marker).exists(),
                "the close callback must return before the driver step marker appears"
            );
            rig.deliver_fixture_frame();
        }
        assert!(
            markers.join(marker).is_file(),
            "the exact body must write its {marker} Escape marker"
        );
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
    assert_eq!(
        text(&summary, "events"),
        "map_down:false:false",
        "the map press is reported for tooltip dismissal"
    );
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
    assert_eq!(text(&summary, "events"), "map_down:false:false");
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
    assert_eq!(text(&summary, "events"), "map_down:false:false");
    assert_eq!(clicks(&summary), 0);
}

const DRAGGABLE_WINDOW: &str = r#"
local windows = require("lib.window")
w = windows.new{title = "Test", content_width = 120, content_height = 60, x = 24, y = 30}
require("lib.panel").new{x = 0, y = 0, width = 20, height = 20, group = w.content}
window.on_opened(function() end)
"#;

#[test]
fn a_window_title_drag_moves_its_frame_and_hit_regions_together() {
    let mut rig = Rig::new(DRAGGABLE_WINDOW);
    let title = rig.key_at(30.0, 35.0).expect("the title is hittable");
    assert!(title.starts_with("window_title_"));

    rig.feed(&[
        down(30.0, 35.0),
        PointerEvent::Move(points(110.0, 115.0)),
        up(110.0, 115.0),
    ]);
    let _ = rig.deliver();

    assert_eq!(rig.key_at(30.0, 35.0), None, "old title bounds are removed");
    assert_eq!(rig.key_at(110.0, 115.0).as_deref(), Some(title.as_str()));
    let content = rig
        .sink
        .quads()
        .into_iter()
        .find(|quad| quad.clip.is_some())
        .expect("the content group has a clipped quad");
    assert_eq!((content.x, content.y), (110.0, 146.0));
    let clip = content.clip.expect("content remains clipped");
    assert_eq!(
        (clip.x, clip.y, clip.width, clip.height),
        (110.0, 146.0, 120.0, 60.0)
    );
}

#[test]
fn a_non_draggable_window_keeps_its_close_button_and_other_windows_still_drag() {
    let script = r#"
local windows = require("lib.window")
local pointer = require("lib.pointer")
local w = windows.new{title = "ledger", content_width = 120, content_height = 60, x = 24, y = 30, draggable = false}
window.on_opened(function()
  assert(w.close_button ~= nil, "draggable=false keeps the close button")
  local function title_event(kind, x, y)
    return {kind = kind, key = w.title_key, x = x, y = y, inside = true}
  end
  pointer.dispatch({overflow = false, held = true, events = {title_event("down", 30, 35)}})
  pointer.dispatch({overflow = false, held = true, events = {title_event("move", 90, 95)}})
  pointer.dispatch({overflow = false, held = false, events = {title_event("up", 90, 95)}})
  local x, y = w:position()
  assert(x == 24 and y == 30, "a non-draggable title does not move the window")
  local close = w.close_button
  local cx, cy = 24 + close.x + close.width / 2, 30 + close.y + close.height / 2
  pointer.dispatch({overflow = false, held = true, events = {{kind = "down", key = close.key, x = cx, y = cy, inside = true}}})
  pointer.dispatch({overflow = false, held = false, events = {{kind = "up", key = close.key, x = cx, y = cy, inside = true}}})
  assert(w.closed, "the close button still closes the non-draggable window")

  local movable = windows.new{title = "Help", content_width = 120, content_height = 60, x = 24, y = 30}
  local function movable_event(kind, x, y)
    return {kind = kind, key = movable.title_key, x = x, y = y, inside = true}
  end
  pointer.dispatch({overflow = false, held = true, events = {movable_event("down", 30, 35)}})
  pointer.dispatch({overflow = false, held = true, events = {movable_event("move", 90, 95)}})
  pointer.dispatch({overflow = false, held = false, events = {movable_event("up", 90, 95)}})
  x, y = movable:position()
  assert(x == 84 and y == 90, "existing windows remain draggable by default")
end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
}

#[test]
fn a_window_title_drag_ends_on_cancel_and_pointer_overflow() {
    let script = r#"
local windows = require("lib.window")
local pointer = require("lib.pointer")
w = windows.new{title = "Test", content_width = 120, content_height = 60, x = 24, y = 30}
window.on_opened(function()
  local function event(kind, x, y)
    return {kind = kind, key = w.title_key, x = x, y = y, inside = true}
  end
  pointer.dispatch({overflow = false, held = true, events = {event("down", 30, 35)}})
  pointer.dispatch({overflow = false, held = false, events = {event("cancel", 30, 35)}})
  pointer.dispatch({overflow = false, held = false, events = {event("move", 80, 80)}})
  local x, y = w:position()
  assert(x == 24 and y == 30, "cancel ends the title drag")

  pointer.dispatch({overflow = false, held = true, events = {event("down", 30, 35)}})
  pointer.dispatch({overflow = true, held = false, events = {}})
  pointer.dispatch({overflow = false, held = false, events = {event("move", 90, 90)}})
  x, y = w:position()
  assert(x == 24 and y == 30, "overflow ends the title drag")
end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
}

#[test]
fn a_fixed_window_has_an_exact_seventeen_pixel_minimum_and_title_click_closes() {
    {
        let script = r#"
local windows = require("lib.window")
local pointer = require("lib.pointer")
w = windows.new{title = "", fixed = true, content_width = require("lib.ui").P, content_height = 20, x = 24, y = 30,
  on_title_click = function() w:close() end}
window.on_opened(function()
  local width = w:size()
  assert(width == 17 * require("lib.ui").P, "fixed windows with empty title and one-pixel content use the exact minimum")
  assert(w.close_button == nil, "fixed windows have no close button")
  local original_x, original_y = w:position()
  local key = w.title_key
  local function event(kind, x, y, inside)
    return {kind = kind, key = key, x = x, y = y, inside = inside}
  end
  pointer.dispatch({overflow = false, held = true, events = {event("down", 30, 35, true)}})
  pointer.dispatch({overflow = false, held = true, events = {event("move", 90, 95, true)}})
  local x, y = w:position()
  assert(x == original_x and y == original_y, "a fixed title does not drag")
  pointer.dispatch({overflow = false, held = false, events = {event("up", 90, 95, false)}})
  assert(not w.closed, "release outside does not close")
  pointer.dispatch({overflow = false, held = true, events = {event("down", 30, 35, true)}})
  pointer.dispatch({overflow = false, held = false, events = {event("cancel", 30, 35, false)}})
  pointer.dispatch({overflow = false, held = false, events = {event("up", 30, 35, true)}})
  assert(not w.closed, "cancel does not leave a title click armed")
  pointer.dispatch({overflow = false, held = true, events = {event("down", 30, 35, true)}})
  pointer.dispatch({overflow = true, held = false, events = {}})
  pointer.dispatch({overflow = false, held = false, events = {event("up", 30, 35, true)}})
  assert(not w.closed, "overflow does not leave a title click armed")
  pointer.dispatch({overflow = false, held = true, events = {event("down", 30, 35, true)}})
  pointer.dispatch({overflow = false, held = false, events = {event("up", 30, 35, true)}})
  assert(w.closed and windows.top() == nil, "a title click closes the fixed window")
end)
"#;
        let mut rig = Rig::new(script);
        rig.opened();
    }
}

#[test]
fn pressing_a_lower_windows_title_raises_it_and_owned_content_still_clicks() {
    let script = r#"
local windows = require("lib.window")
local button = require("lib.button")
first = windows.new{title = "First", content_width = 120, content_height = 60, x = 24, y = 30}
button.new{x = 0, y = 0, text = "Go", group = first.content, owner = first, on_click = click}
second = windows.new{title = "Second", content_width = 120, content_height = 60, x = 150, y = 30}
window.on_opened(function() assert(windows.top() == first, "pressing the lower title raises its window") end)
"#;
    let mut rig = Rig::new(script);
    rig.feed(&[down(30.0, 35.0), up(30.0, 35.0)]);
    let _ = rig.deliver();
    rig.opened();

    let quads = rig.sink.quads();
    let near = |left: f64, right: f64| (left - right).abs() < 0.01;
    let first_frame = quads
        .iter()
        .position(|quad| {
            near(quad.x, 24.0)
                && near(quad.y, 30.0)
                && near(quad.width, 16.0)
                && near(quad.height, 16.0)
        })
        .expect("first window chrome quad (the body frame's top-left tile)");
    let second_frame = quads
        .iter()
        .position(|quad| {
            near(quad.x, 150.0)
                && near(quad.y, 30.0)
                && near(quad.width, 16.0)
                && near(quad.height, 16.0)
        })
        .expect("second window chrome quad (the body frame's top-left tile)");
    assert!(
        first_frame > second_frame,
        "raising the overlapping lower window moves its chrome above the other"
    );

    let origin_x = 24.0 + 6.0;
    let origin_y = 30.0 + 30.0 + 6.0;
    let point = (origin_x + 5.0, origin_y + 5.0);
    assert!(
        rig.key_at(point.0, point.1)
            .is_some_and(|key| key.starts_with("button_")),
        "the content button is published at its window-relative origin"
    );
    rig.feed(&[down(point.0, point.1), up(point.0, point.1)]);
    let summary = rig.deliver();
    assert_eq!(
        clicks(&summary),
        1,
        "the owned content button receives the click"
    );
}

#[test]
fn window_geometry_updates_and_clamps_match_the_p2_contract() {
    let script = r#"
local windows = require("lib.window")
local panel = require("lib.panel")
windows.update_viewport(800, 600)
w = windows.new{title = "A", content_width = 120, content_height = 60, x = 100, y = 100}
panel.new{x = 0, y = 0, width = 20, height = 20, group = w.content}
window.on_opened(function()
  local width, height = w:size()
  assert(width == 132 and height == 106, "P2 short-title outer size is literal")
  local x, y = w:content_origin()
  assert(x == 106 and y == 136, "content origin includes the title and padding")

  w:move_to(-999, 9999)
  x, y = w:position()
  assert(x == -100 and y == 570, "left and bottom clamp")
  w:move_to(9999, -999)
  x, y = w:position()
  assert(x == 768 and y == 0, "right and top clamp")

  windows.update_viewport(20, 10)
  x, y = w:position()
  assert(x == -12 and y == 0, "the small viewport uses non-inverted bounds")
  windows.update_viewport(1, 1)
  x, y = w:position()
  assert(x == -32 and y == 0, "the one-point viewport remains finite")
  windows.update_viewport(800, 600)
  x, y = w:position()
  assert(x == -32 and y == 0, "the window returns within the ordinary clamp")

  w:move_to(100, 100)
  w:set_title("Long title")
  width, height = w:size()
  x, y = w:position()
  assert(width == 150 and height == 106 and x == 100 and y == 100,
    "long title widens without moving the top-left")
  w:set_content_size(50, 40)
  width, height = w:size()
  x, y = w:position()
  assert(width == 150 and height == 86 and x == 100 and y == 100,
    "content resize updates outer size and preserves the position")
end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
    let content = rig
        .sink
        .quads()
        .into_iter()
        .find(|quad| quad.clip.is_some())
        .expect("the resized content group has a clipped quad");
    let clip = content.clip.expect("content clip");
    assert_eq!(
        (clip.x, clip.y, clip.width, clip.height),
        (106.0, 136.0, 50.0, 40.0)
    );
    let regions = rig
        .harness
        .bridge
        .hit_regions()
        .expect("updated hit regions");
    let rect = |key: &str| {
        regions
            .regions()
            .iter()
            .find(|region| region.key() == key)
            .expect("updated window key")
            .rect()
    };
    assert_eq!(
        rect("window_frame_1"),
        HitRect::from_points(100.0, 100.0, 150.0, 86.0).unwrap()
    );
    assert_eq!(
        rect("window_title_2"),
        HitRect::from_points(100.0, 100.0, 128.0, 30.0).unwrap()
    );
    assert_eq!(
        rect("button_3"),
        HitRect::from_points(228.0, 100.0, 20.0, 30.0).unwrap()
    );
}

#[test]
fn window_chrome_content_and_hit_rectangles_match_the_p2_literals() {
    let script = r#"
local windows = require("lib.window")
local label = require("lib.label")
w = windows.new{title = "A", content_width = 120, content_height = 60, x = 24, y = 30}
label.new{x = 0, y = 0, text = "A", group = w.content}
window.on_opened(function() end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();

    let quads = rig.sink.quads();
    // The chrome in painter order: the body frame (cut 7, both corners: 66 by 53 UI pixels), the title bar (cut 7, top-left, 66 by 15),
    // the title "A" at 8 UI pixels, and the close button (a cut-3 frame of 10 by 15 and its glyph).
    let chrome: Vec<(f64, f64, f64, f64)> = quads[..32]
        .iter()
        .map(|quad| (quad.x, quad.y, quad.width, quad.height))
        .collect();
    let expected = [
        // the body: fill tiles, bands, line tiles, edges
        (24.0, 30.0, 16.0, 16.0),
        (140.0, 120.0, 16.0, 16.0),
        (40.0, 32.0, 114.0, 14.0),
        (26.0, 46.0, 128.0, 74.0),
        (26.0, 120.0, 114.0, 14.0),
        (24.0, 30.0, 16.0, 16.0),
        (140.0, 120.0, 16.0, 16.0),
        (40.0, 30.0, 116.0, 2.0),
        (154.0, 32.0, 2.0, 88.0),
        (26.0, 134.0, 114.0, 2.0),
        (24.0, 46.0, 2.0, 90.0),
        // the title bar: fill tile, bands, line tile, edges
        (24.0, 30.0, 16.0, 16.0),
        (40.0, 32.0, 114.0, 14.0),
        (26.0, 46.0, 128.0, 12.0),
        (24.0, 30.0, 16.0, 16.0),
        (40.0, 30.0, 116.0, 2.0),
        (154.0, 32.0, 2.0, 28.0),
        (26.0, 58.0, 128.0, 2.0),
        (24.0, 46.0, 2.0, 14.0),
        // the title: at x = 8 UI pixels (16 points) in
        (40.0, 34.0, 10.0, 22.0),
        // the close button's frame
        (134.0, 30.0, 8.0, 8.0),
        (146.0, 52.0, 8.0, 8.0),
        (142.0, 32.0, 10.0, 6.0),
        (136.0, 38.0, 16.0, 14.0),
        (136.0, 52.0, 10.0, 6.0),
        (134.0, 30.0, 8.0, 8.0),
        (146.0, 52.0, 8.0, 8.0),
        (142.0, 30.0, 12.0, 2.0),
        (152.0, 32.0, 2.0, 20.0),
        (136.0, 58.0, 10.0, 2.0),
        (134.0, 38.0, 2.0, 22.0),
        // and its glyph, centred in the button
        (138.0, 34.0, 10.0, 22.0),
    ];
    assert_eq!(chrome, expected);
    let content = quads
        .iter()
        .find(|quad| quad.clip.is_some())
        .expect("content label quad");
    assert_eq!((content.x, content.y), (30.0, 66.0));
    assert_eq!(
        content.clip.unwrap(),
        ck_render_2d::PointRect {
            x: 30.0,
            y: 66.0,
            width: 120.0,
            height: 60.0
        }
    );

    let regions = rig.harness.bridge.hit_regions().unwrap();
    let rect = |key: &str| {
        regions
            .regions()
            .iter()
            .find(|region| region.key() == key)
            .unwrap()
            .rect()
    };
    assert_eq!(
        rect("window_frame_1"),
        HitRect::from_points(24.0, 30.0, 132.0, 106.0).unwrap()
    );
    assert_eq!(
        rect("window_title_2"),
        HitRect::from_points(24.0, 30.0, 110.0, 30.0).unwrap()
    );
    assert_eq!(
        rect("button_3"),
        HitRect::from_points(134.0, 30.0, 20.0, 30.0).unwrap()
    );
}

#[test]
fn a_place_windows_chrome_has_the_cyan_outline_the_amber_divider_and_dark_title_text_and_a_window_has_no_divider()
 {
    let tint =
        |red: u8, green: u8, blue: u8| ck_render_2d::Tint::from_straight(red, green, blue, 255);
    let (body, cyan, amber, dark, red, light) = (
        tint(0x15, 0x11, 0x0a),
        tint(0x4f, 0xb3, 0xc8),
        tint(0xc9, 0x93, 0x2e),
        tint(0x15, 0x11, 0x0a),
        tint(0x6e, 0x13, 0x0f),
        tint(0xec, 0xe3, 0xcb),
    );
    let script = |kind: &str| {
        format!(
            r#"
local windows = require("lib.window")
w = windows.new{{title = "A", kind = "{kind}", content_width = 120, content_height = 60, x = 24, y = 30}}
window.on_opened(function() end)
"#
        )
    };
    // A place window (P = 2): the body frame (11 quads) fills near-black and outlines cyan; the title bar (8 quads) is cyan; the divider
    // (quad 19) is one amber UI pixel high on the row under the title bar, from the second to the second-to-last column; the title is
    // dark; the close button is a title-style button: cyan, with a dark glyph.
    let mut rig = Rig::new(&script("place"));
    rig.opened();
    let quads = rig.sink.quads();
    for (index, quad) in quads[..11].iter().enumerate() {
        assert_eq!(
            quad.tint,
            if index < 5 { body } else { cyan },
            "body quad {index}"
        );
    }
    for (index, quad) in quads[11..19].iter().enumerate() {
        assert_eq!(quad.tint, cyan, "title bar quad {index}");
    }
    let divider = &quads[19];
    assert_eq!(
        (divider.x, divider.y, divider.width, divider.height),
        (26.0, 60.0, 128.0, 2.0)
    );
    assert_eq!(divider.tint, amber);
    assert_eq!(quads[20].tint, dark, "the title");
    for (index, quad) in quads[21..32].iter().enumerate() {
        assert_eq!(quad.tint, cyan, "close button frame quad {index}");
    }
    assert_eq!(quads[32].tint, dark, "the close button's glyph");
    assert_eq!(quads.len(), 33);
    // A window of the default kind: the red outline and no divider; the title and the close glyph are the text colour on the red bar.
    let mut rig = Rig::new(&script("window"));
    rig.opened();
    let quads = rig.sink.quads();
    for (index, quad) in quads[..11].iter().enumerate() {
        assert_eq!(
            quad.tint,
            if index < 5 { body } else { red },
            "body quad {index}"
        );
    }
    assert_eq!(
        quads[19].tint, light,
        "the title follows the title bar directly: no divider quad"
    );
    assert_eq!(quads.len(), 32);
}

#[test]
fn an_unowned_key_does_not_raise_a_window_and_escape_closes_only_the_top() {
    let script = r#"
local windows = require("lib.window")
local button = require("lib.button")
first = windows.new{title = "First", content_width = 120, content_height = 60, x = 24, y = 30}
second = windows.new{title = "Second", content_width = 120, content_height = 60, x = 150, y = 30}
button.new{x = 310, y = 200, text = "Free", on_click = function()
  assert(windows.top() == second, "an unowned key does not raise a window")
  click()
end}
window.on_opened(function()
  assert(windows.top() == second)
end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
    rig.feed(&[down(315.0, 205.0), up(315.0, 205.0)]);
    let summary = rig.deliver();
    assert_eq!(clicks(&summary), 1);

    let escape = r#"
local windows = require("lib.window")
a = windows.new{title = "A", content_width = 20, content_height = 20, x = 10, y = 10}
b = windows.new{title = "B", content_width = 20, content_height = 20, x = 40, y = 10}
window.on_opened(function()
  assert(windows.top() == b)
  assert(windows.close_request("key") == false and b.closed and windows.top() == a,
    "Escape closes the top and leaves the other window")
  assert(windows.close_request("key") == false and a.closed and windows.top() == nil,
    "second Escape closes the remaining window")
  assert(windows.close_request("key") == true, "Escape with no windows requests application close")
  assert(windows.close_request("button") == true, "the OS close button always requests application close")
end)
"#;
    let mut rig = Rig::new(escape);
    rig.opened();
}

#[test]
fn escape_closes_the_top_tooltip_then_the_open_list_bar_picker_then_the_top_window_one_each() {
    use super::listbar_tests::{MARKS, PROBE, frame, hover, point, rig as listbar_rig};
    // A locked tooltip (hovered over its source long enough, then left), an open picker and a window: three presses close them in
    // that order, and a fourth asks to quit.
    let mut rig = listbar_rig(
        &format!("new_window(120, 100) new_bar() {MARKS} {PROBE}"),
        &[
            r#"
bar:view({row("a", "ringfort", 1), row("b", "crannog", 2)})
mark("probe", 630, 420)
"#,
            r#"
eq(count_hits("tooltip_panel_"), 1, "the tooltip is open")
bar.filter_button.on_click()
expect(bar.picker ~= nil and not W.closed)
"#,
            r#"
eq(count_hits("tooltip_panel_"), 0, "the first Escape closed the tooltip")
expect(bar.picker ~= nil and not W.closed, "and nothing else")
"#,
            r#"
expect(bar.picker == nil and not W.closed, "the second closed the picker, not the window")
"#,
            r#"
expect(W.closed, "the third closed the window")
"#,
        ],
    );
    rig.opened();
    let probe = point(&rig, "probe");
    hover(&mut rig, probe);
    for _ in 0..150 {
        frame(&mut rig);
    }
    hover(&mut rig, (700.0, 550.0));
    rig.opened();
    assert!(rig.escape(), "Escape closes the tooltip");
    rig.opened();
    assert!(rig.escape(), "Escape closes the picker");
    rig.opened();
    assert!(rig.escape(), "Escape closes the window");
    rig.opened();
    assert!(!rig.escape(), "with nothing else open, Escape asks to quit");
}

#[test]
fn the_production_place_panel_updates_in_place_and_help_raises_on_a_second_click() {
    let script = format!(
        r"
callbacks = {{}}
local windows = require('lib.window')
local lists = require('lib.list')
-- The panel's lists, as they are created: a second selection must update the one panel, not open another.
created_lists = 0
local actual_list_new = lists.new
lists.new = function(spec)
  created_lists = created_lists + 1
  return actual_list_new(spec)
end
game = {{
  on_time = function(callback) callbacks.time = callback end,
  on_select = function(callback) callbacks.select = callback end,
  on_hover = function(callback) callbacks.hover = callback end,
  control = function() end,
  place = function(id, revision)
    return {{id = id, workers = 0, dependents = 0, food_days = 0, footprint_m2 = 0, prepared_m2 = 0, max_buildable_m2 = 0, storage_used_l = 0, storage_capacity_l = 0,
      overflow_lost = 0, prices = {{}}, pops = {{}}, buildings = {{}}}}
  end,
}}
{} 
callbacks.time({{day = 1, month = 1, year = 768, hour = 0, paused = true, speed = 1, economy_revision = 1}})
callbacks.select({{id = 'place-a', name = 'Place A', kind = 'town', size = 2}})
place = windows.top()
place:move_to(200, 200)
callbacks.select({{id = 'place-b', name = 'A Much Longer Place', kind = 'city', size = 7}})
assert(windows.top() == place, 'the second selection updates and raises the existing place window')
assert(place.title == 'A Much Longer Place', 'the update gives the window the selected place name')
assert(created_lists == 1, 'the update refreshes the one panel list and opens no other')
local x, y = place:position()
assert(x == 200 and y == 200, 'place update preserves its top-left')
local stage = 1
local help
frame.on_update(function()
  if stage == 1 then
    place:raise()
  elseif stage == 2 then
    help = windows.top()
    place:raise()
  elseif stage == 3 then
    assert(windows.top() == help, 'a second Help click raises the existing Help window')
    help:close()
    assert(windows.top() == place, 'closing Help reveals the one updated place window')
    place:close()
    assert(windows.top() == nil, 'there is no duplicate place window')
  end
  stage = stage + 1
end)
",
        include_str!("../../../../scripts/ui/main.lua")
    );
    let mut rig = Rig::new(&script);
    rig.opened();
    let regions = rig.harness.bridge.hit_regions().expect("published hits");
    // The buttons in creation order: the three transports, Help (`?`), `places`, and the three map modes'.
    let buttons: Vec<String> = regions
        .regions()
        .iter()
        .take_while(|region| !region.key().starts_with("window_frame_"))
        .filter(|region| region.key().starts_with("button_"))
        .map(|region| region.key().to_owned())
        .collect();
    let help_key = buttons
        .iter()
        .rev()
        .nth(5)
        .cloned()
        .expect("the production time bar has a Help button");
    let point = (0..400)
        .flat_map(|y| (0..400).map(move |x| (f64::from(x) + 0.5, f64::from(y) + 0.5)))
        .find(|(x, y)| regions.key_at(points(*x, *y)) == Some(help_key.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "the Help button {help_key} has a point in the viewport; regions: {:?}",
                regions
                    .regions()
                    .iter()
                    .map(ck_render_2d::HitRegion::key)
                    .collect::<Vec<_>>()
            )
        });
    rig.feed(&[down(point.0, point.1), up(point.0, point.1)]);
    let _ = rig.deliver();
    rig.feed(&[down(point.0, point.1), up(point.0, point.1)]);
    let _ = rig.deliver();
    let _ = rig.deliver();
}

#[test]
fn a_close_button_click_closes_but_dragging_off_the_close_button_does_not() {
    let script = r#"
local windows = require("lib.window")
w = windows.new{title = "A", content_width = 120, content_height = 60, x = 24, y = 30}
window.on_opened(function()
  local x = 134
  local y = 35
  pointer.dispatch({overflow = false, held = true, events = {{kind = "down", key = w.close_button.key, x = x, y = y, inside = true}}})
  pointer.dispatch({overflow = false, held = false, events = {{kind = "up", key = w.close_button.key, x = 100, y = 60, inside = false}}})
  assert(not w.closed, "press then drag off does not close")
  pointer.dispatch({overflow = false, held = true, events = {{kind = "down", key = w.close_button.key, x = x, y = y, inside = true}}})
  pointer.dispatch({overflow = false, held = false, events = {{kind = "up", key = w.close_button.key, x = x, y = y, inside = true}}})
  assert(w.closed and windows.top() == nil, "clicking the close button destroys the window")
end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
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
loose = panel.new{x = 4, y = 6, width = 20, height = 20}
inside = panel.new{x = 4, y = 6, width = 20, height = 20, group = g}
label.new{x = 20, y = 20, text = "A", group = g}
a = button.new{x = 10, y = 10, text = "Go", on_click = click, group = g}
window.on_opened(function() end)
"#;
    let mut rig = Rig::new(script);
    rig.opened();
    let frame = rig.sink.quads();
    let xs: Vec<(f64, f64)> = frame.iter().map(|quad| (quad.x, quad.y)).collect();
    // Painter order: the base group first (the loose panel's eleven quads), then the group in its own creation order: the panel's
    // eleven, the label's glyph and the button's eleven and two glyphs. Every grouped quad is translated by the offset; the loose one
    // is not.
    assert_eq!(xs[0], (4.0, 6.0), "the quad without a group");
    assert_eq!(xs[11], (104.0, 56.0), "the grouped panel");
    assert_eq!(xs[22], (120.0, 70.0), "the grouped label's glyph");
    assert_eq!(xs[23], (110.0, 60.0), "the button's frame");
    assert_eq!(frame.len(), 11 + 11 + 1 + 11 + 2);
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
        ui_scale_k: None,
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

/// A button "Go" (32 by 30 points) at `local` in a group offset by `offset`, in the state `state`: the eleven quads of its frame.
fn frame_quads(offset: (f64, f64), local: (f64, f64), state: &str) -> Vec<FrameQuad> {
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
    rig.sink.quads()[..11].to_vec()
}

/// The rectangle of a quad, in points.
fn rect_of(quad: &FrameQuad) -> (f64, f64, f64, f64) {
    (quad.x, quad.y, quad.width, quad.height)
}

/// The bounding box of the quads' rectangles: `(x, y, width, height)`.
fn bounds(quads: &[FrameQuad]) -> (f64, f64, f64, f64) {
    let left = quads.iter().map(|quad| quad.x).fold(f64::MAX, f64::min);
    let top = quads.iter().map(|quad| quad.y).fold(f64::MAX, f64::min);
    let right = quads
        .iter()
        .map(|quad| quad.x + quad.width)
        .fold(f64::MIN, f64::max);
    let bottom = quads
        .iter()
        .map(|quad| quad.y + quad.height)
        .fold(f64::MIN, f64::max);
    (left, top, right - left, bottom - top)
}

#[test]
fn a_frame_is_placed_by_the_ordinary_rule_at_the_domains_bounds_and_nothing_of_it_is_parked() {
    let edge = POINT_DOMAIN;
    // The button is laid out so that its hit rectangle stays inside the domain under the group's offset.
    let cases = [
        ((edge, edge), (-60.0, -60.0)),
        ((-edge, -edge), (60.0, 60.0)),
        ((0.0, 0.0), (10.0, 10.0)),
    ];
    for (offset, local) in cases {
        for state in ["normal", "hover", "pressed"] {
            let quads = frame_quads(offset, local, state);
            assert_eq!(quads.len(), 11);
            // The frame's quads cover the button's rectangle exactly, translated by the group's offset.
            assert_eq!(
                bounds(&quads),
                (offset.0 + local.0, offset.1 + local.1, 32.0, 30.0),
                "{offset:?} {state}"
            );
            for quad in &quads {
                assert_eq!(
                    placed(quad),
                    expected_destination(rect_of(quad), None),
                    "{offset:?} {state}: a quad of the frame by the ordinary rule"
                );
            }
        }
    }
}

#[test]
fn at_a_normal_position_the_whole_frame_shows_in_each_state_with_the_look_of_the_state() {
    let tint =
        |red: u8, green: u8, blue: u8| ck_render_2d::Tint::from_straight(red, green, blue, 255);
    for (state, fill, line) in [
        ("normal", tint(0x15, 0x11, 0x0a), tint(0x6e, 0x13, 0x0f)),
        ("hover", tint(0x15, 0x11, 0x0a), tint(0xa8, 0x26, 0x1c)),
        ("pressed", tint(0xa8, 0x26, 0x1c), tint(0xa8, 0x26, 0x1c)),
    ] {
        let quads = frame_quads((100.0, 50.0), (20.0, 10.0), state);
        assert!(
            quads.iter().all(|quad| placed(quad).is_some()),
            "{state}: every quad of the frame is drawn"
        );
        assert_eq!(bounds(&quads), (120.0, 60.0, 32.0, 30.0), "{state}");
        // Five fill quads, then six line quads, in the colours of the state.
        for (index, quad) in quads.iter().enumerate() {
            let want = if index < 5 { fill } else { line };
            assert_eq!(quad.tint, want, "{state}: quad {index}");
        }
    }
}

#[test]
fn a_frame_follows_a_group_clip_quad_by_quad() {
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
    let quads = rig.sink.quads()[..11].to_vec();
    let clip = Some((110.0, 60.0, 16.0, 30.0));
    assert_eq!(bounds(&quads), (110.0, 60.0, 32.0, 30.0));
    for quad in &quads {
        assert_eq!(
            placed(quad),
            expected_destination(rect_of(quad), clip),
            "{:?}",
            rect_of(quad)
        );
    }
    // The clip keeps the left half of the button: the quads wholly right of it are never drawn, the others are cut to it.
    let drawn = quads.iter().filter(|quad| placed(quad).is_some()).count();
    assert!(drawn > 0 && drawn < 11, "{drawn} of 11 quads are drawn");
    assert_eq!(placed(&quads[0]), Some((220, 120, 236, 136)));
    assert_eq!(placed(&quads[1]), None);
}

/// The metrics of a window opened in pixels at the UI scale 1.5: 1920 by 1200 pixels (the least that allows it), 960 by 600 points, drawn at
/// 1.5 pixels a UI unit (`k` = 3).
fn metrics_at_one_and_a_half() -> Metrics {
    Metrics {
        points_width: 960,
        points_height: 600,
        pixels: PixelSize {
            width: 1920,
            height: 1200,
        },
        ui_scale_k: Some(3),
    }
}

/// Where a quad is drawn at the UI scale 1.5: each edge is the rounded product of its unit coordinate and 1.5 (the existing `edges` rule),
/// then cut to the window.
fn placed_at_one_and_a_half(quad: &FrameQuad) -> Option<(i64, i64, i64, i64)> {
    quad.place(&metrics_at_one_and_a_half()).map(|placed| {
        let d = placed.destination;
        (
            i64::from(d.left),
            i64::from(d.top),
            i64::from(d.right),
            i64::from(d.bottom),
        )
    })
}

/// The go button (32 by 30 units at (10, 20), as at 2 units a UI pixel) in a window opened in pixels at the UI scale 1.5 is drawn with its
/// top-left at pixels (15, 30) and each of its sizes 1.5 times its unit size: 48 by 45 pixels. (The accepted 3-points-a-pixel rectangle,
/// snapped to (9, 21), is not reproduced: that scale no longer exists.)
#[test]
fn a_button_at_the_ui_scale_one_and_a_half_is_drawn_at_one_and_a_half_pixels_a_unit() {
    let quads = frame_quads((0.0, 0.0), (10.0, 20.0), "normal");
    let drawn: Vec<_> = quads.iter().filter_map(placed_at_one_and_a_half).collect();
    assert_eq!(drawn.len(), quads.len(), "every quad of the frame shows");
    let left = drawn.iter().map(|rect| rect.0).min().expect("quads");
    let top = drawn.iter().map(|rect| rect.1).min().expect("quads");
    let right = drawn.iter().map(|rect| rect.2).max().expect("quads");
    let bottom = drawn.iter().map(|rect| rect.3).max().expect("quads");
    assert_eq!((left, top), (15, 30), "the top-left is 1.5 times (10, 20)");
    assert_eq!(
        (right - left, bottom - top),
        (48, 45),
        "32 by 30 units, 1.5 pixels each"
    );
}

/// Every drawn edge of a widget on the UI-pixel grid (two units) is a multiple of 3 pixels at the UI scale 1.5: the old "multiple of the
/// pixel scale" check, now over drawn pixels.
#[test]
fn every_drawn_edge_of_a_ui_pixel_aligned_widget_is_a_multiple_of_three_pixels_at_scale_one_and_a_half()
 {
    let script = r#"
local button = require("lib.button")
local label = require("lib.label")
local panel = require("lib.panel")
local ui = require("lib.ui")
local b = button.new{x = 14, y = 18, text = "Hi", on_click = function() end}
b:move(30, 30)
b:set_text("Hello")
panel.new{x = 6, y = 10, width = 56, height = 56}:set_rect(12, 14, 48, 50)
local l = label.new{x = 2, y = 2, text = "ab"}
l:move(24, 42)
local x, y = ui.anchor("bottom_right", 48, 46, -8, -8, 800, 600)
panel.new{x = x, y = y, width = 48, height = 46}
"#;
    let mut rig = Rig::new(script);
    rig.opened();
    let mut seen = 0;
    for quad in rig.sink.quads() {
        let Some((left, top, right, bottom)) = placed_at_one_and_a_half(&quad) else {
            continue;
        };
        for edge in [left, top, right, bottom] {
            assert_eq!(
                edge % 3,
                0,
                "{:?} placed at ({left}, {top}, {right}, {bottom})",
                rect_of(&quad)
            );
            seen += 1;
        }
    }
    assert!(seen > 40, "checked {seen} edges");
}
