//! The UI script (`scripts/ui/main.lua`) and its widget library (`scripts/ui/lib/`), run for real over the shipped pixel font and a
//! scene that only records what it is asked to do: the text helpers, the date readout, the hover tooltip and the idle rule.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub(crate) const PLACE_TOOLTIP_TEST_FIXTURE: &str = r#"
require("lib.classes").set_revision(0)
game.places = function() return {{id = "town", name = "Alpha", kind = "town", size = 1}} end
game.place = function(id, revision)
  if id ~= "town" or revision ~= 0 then return nil end
  return {id = id, workers = 0, dependents = 0, pops = {},
    land = {usable = {}, land = {}, sea = 0},
    last_month = {closed = true, produced = {}, used_by_pops = {}, lost = {}}}
end
"#;

const TWO_PLACE_TOOLTIP_TEST_FIXTURE: &str = r#"
require("lib.classes").set_revision(0)
game.places = function()
  return {
    {id = "town", name = "Alpha", kind = "town", size = 1},
    {id = "beta", name = "Beta", kind = "town", size = 1},
  }
end
game.place = function(id, revision)
  if (id ~= "town" and id ~= "beta") or revision ~= 0 then return nil end
  return {id = id, workers = 0, dependents = 0, pops = {},
    land = {usable = {}, land = {}, sea = 0},
    last_month = {closed = true, produced = {}, used_by_pops = {}, lost = {}}}
end
"#;

use ck_lua::{CallOutcome, ConfigValue, Extension, HostFunction, Modules, VmOwner, VmSettings};
use ck_render_2d::{Filter, QuadSink, SceneBridge, Snapshot};
use ck_resources::{Assets, Mode, ResourceLimits, ResourceService, ServiceWakes};
use ck_window::PixelSize;
use ck_window::{Notifier, Requests, WindowDesc};

use crate::config::TooltipsConfig;
use crate::frame_api::{self, FrameBackend, FrameRegistration};
use crate::game::core::Message;
use crate::game::slot::{PauseReason, SimSnapshot};
use crate::testing::Dir;
use crate::window_api::FRAME_UPDATE;
use crate::{font, game_api, window_api};

type Log = Arc<Mutex<Vec<String>>>;
type SceneCalls = Arc<Mutex<Vec<ObservedHostCall>>>;

#[derive(Clone, Debug, PartialEq)]
struct ObservedHostCall {
    name: String,
    arguments: Vec<Option<ConfigValue>>,
    result: Vec<ConfigValue>,
}

fn show(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Integer(number) => number.to_string(),
        ConfigValue::Float(number) => number.to_string(),
        ConfigValue::String(text) => text.clone(),
        ConfigValue::Bool(flag) => flag.to_string(),
        other => format!("{other:?}"),
    }
}

fn field(table: &ConfigValue, name: &str) -> String {
    match table {
        ConfigValue::Table(fields) => fields.get(name).map(show).unwrap_or_default(),
        _ => String::new(),
    }
}

pub(crate) fn push(log: &Log, line: String) {
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(line);
}

#[test]
fn a_text_line_keeps_its_font_choice_and_four_argument_lines_stay_standard() {
    let readout = Readout::script(
        r"
local text = require('lib.text')
local small = text.line(0, 0, nil, nil, 'small')
text.set(small, 'oats')
local standard = text.line(0, 20)
text.set(standard, 'a')
        ",
    );
    let creates: Vec<_> = readout
        .entries()
        .into_iter()
        .filter(|line| line.starts_with("create "))
        .collect();
    assert!(
        creates
            .iter()
            .any(|line| line == "create q1 ui/font/small/U+006F.png 0 0 8 18"),
        "small font's first glyph and 9 * P height: {creates:?}"
    );
    assert!(
        creates
            .iter()
            .any(|line| line == "create q5 ui/font/glyphs/U+0061.png 0 20 10 22"),
        "four-argument line remains standard at 11 * P height: {creates:?}"
    );
}

/// A function that records `<name> <arguments>` and returns nothing.
pub(crate) fn recording(log: &Log, name: &'static str) -> HostFunction {
    let log = Arc::clone(log);
    Arc::new(move |arguments| {
        let arguments: Vec<String> = arguments.iter().flatten().map(show).collect();
        push(
            &log,
            format!("{name} {}", arguments.join(" "))
                .trim_end()
                .to_owned(),
        );
        Ok(Vec::new())
    })
}

#[derive(Default)]
struct Names {
    quads: HashMap<String, String>,
    groups: HashMap<String, String>,
    textures: HashMap<String, String>,
    quad_count: usize,
    group_count: usize,
    foreign_quad: String,
    foreign_group: String,
    foreign_texture: String,
}

fn test_bridge() -> Arc<SceneBridge> {
    struct Sink;
    impl QuadSink for Sink {
        fn publish(&self, _snapshot: Snapshot) -> Option<Snapshot> {
            None
        }
    }
    let bridge = SceneBridge::new(Filter::Nearest);
    bridge
        .attach(
            &WindowDesc {
                title: "UI rig".to_owned(),
                width: 200,
                height: 100,
                clear: [0.0; 3],
                ..WindowDesc::default()
            },
            PixelSize {
                width: 200,
                height: 100,
            },
            Arc::new(Sink),
        )
        .expect("the readout scene attaches");
    bridge
}

struct StrictScene {
    calls: HashMap<&'static str, HostFunction>,
    texture_load: HostFunction,
    names: Arc<Mutex<Names>>,
    log: Log,
    tints: Log,
    observer: SceneCalls,
}

impl StrictScene {
    fn new(log: &Log, tints: &Log, assets: &std::path::Path, observer: SceneCalls) -> Self {
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            Mode::Headless,
            ServiceWakes::default(),
            Some(Assets::new(assets.to_path_buf())),
        )
        .expect("the readout resource service");
        let bridge = test_bridge();
        let writer = bridge.take_writer().expect("the readout scene writer");
        let (calls, _end) = crate::render_api::test_calls(
            Some(assets.to_path_buf()),
            writer,
            None,
            service.clone(),
        );
        let calls: HashMap<_, _> = calls.into_iter().collect();

        // Unknown rig names use valid handles from a different registry/service. The real API supplies the refusal.
        let mut foreign_writer = test_bridge().take_writer().expect("a foreign scene writer");
        let foreign_group = foreign_writer
            .create_group()
            .expect("a foreign group")
            .token()
            .to_string();
        let texture_key = service
            .load("ui/font/glyphs/U+0061.png")
            .expect("a texture key");
        let texture_ref = service.retain(texture_key).expect("a texture reference");
        let foreign_quad = foreign_writer
            .create_quad(texture_ref, 0.0, 0.0, 1.0, 1.0)
            .expect("a foreign quad")
            .token()
            .to_string();
        let foreign_service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            Mode::Headless,
            ServiceWakes::default(),
            Some(Assets::new(assets.to_path_buf())),
        )
        .expect("the foreign resource service");
        let foreign_texture = foreign_service
            .load("ui/font/glyphs/U+0061.png")
            .expect("a foreign texture")
            .token()
            .to_string();
        let names = Arc::new(Mutex::new(Names {
            foreign_quad,
            foreign_group,
            foreign_texture,
            ..Names::default()
        }));

        Self {
            calls,
            texture_load: crate::texture_api::test_load_call(service),
            names,
            log: Arc::clone(log),
            tints: Arc::clone(tints),
            observer,
        }
    }

    fn call(
        &self,
        name: &str,
        arguments: &[Option<ConfigValue>],
    ) -> Result<Vec<ConfigValue>, String> {
        if name == "texture.load" {
            let result = (self.texture_load)(arguments)?;
            let result = record_load(arguments, &result, &self.names, &self.log);
            self.observer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(ObservedHostCall {
                    name: name.to_owned(),
                    arguments: arguments.to_vec(),
                    result: result.clone(),
                });
            return Ok(result);
        }
        let real = self
            .calls
            .get(name)
            .ok_or_else(|| format!("unknown test call {name}"))?;
        let translated = translate(name, arguments, &self.names);
        let result = real(&translated)?;
        let result = record_success(
            name,
            arguments,
            &result,
            &self.names,
            &self.log,
            &self.tints,
        )?;
        self.observer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ObservedHostCall {
                name: format!("render.{name}"),
                arguments: arguments.to_vec(),
                result: result.clone(),
            });
        Ok(result)
    }

    fn call_real(
        &self,
        name: &str,
        arguments: &[Option<ConfigValue>],
    ) -> Result<Vec<ConfigValue>, String> {
        if name == "texture.load" {
            return (self.texture_load)(arguments);
        }
        let real = self
            .calls
            .get(name)
            .ok_or_else(|| format!("unknown test call {name}"))?;
        real(&translate(name, arguments, &self.names))
    }

    fn extensions(self: &Arc<Self>) -> Vec<Extension> {
        let mut render = Extension::new("render");
        for name in [
            "create_group",
            "raise_groups",
            "create_quad",
            "set_hit",
            "set_position",
            "set_rect",
            "set_tint",
            "set_group_clip",
            "set_group_offset",
            "set_group_visible",
            "destroy_group",
            "destroy_quad",
        ] {
            let scene = Arc::clone(self);
            render = render.function(name, Arc::new(move |arguments| scene.call(name, arguments)));
        }
        let scene = Arc::clone(self);
        let texture = Extension::new("texture").function(
            "load",
            Arc::new(move |arguments| scene.call("texture.load", arguments)),
        );
        vec![render, texture]
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "HostFunction arguments use Option to represent Lua nil"
)]
fn value(value: ConfigValue) -> Option<ConfigValue> {
    Some(value)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "HostFunction arguments use Option to represent Lua nil"
)]
fn string(value: &str) -> Option<ConfigValue> {
    Some(ConfigValue::String(value.to_owned()))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "HostFunction arguments use Option to represent Lua nil"
)]
fn number(value: f64) -> Option<ConfigValue> {
    Some(ConfigValue::Float(value))
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> ConfigValue {
    ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Float(x)),
        ("y".to_owned(), ConfigValue::Float(y)),
        ("width".to_owned(), ConfigValue::Float(width)),
        ("height".to_owned(), ConfigValue::Float(height)),
    ]))
}

fn test_strict_scene() -> (StrictScene, Log, Log) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let tints = Arc::new(Mutex::new(Vec::new()));
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    (
        StrictScene::new(&log, &tints, &assets, Arc::default()),
        log,
        tints,
    )
}

fn assert_refusal_matches_real(
    scene: &StrictScene,
    name: &str,
    arguments: &[Option<ConfigValue>],
    log: &Log,
    tints: &Log,
) {
    let before_log = log.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let before_tints = tints.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let standin = scene.call(name, arguments);
    let real = scene.call_real(name, arguments);
    let (Err(standin_error), Err(real_error)) = (standin, real) else {
        panic!("{name} {arguments:?}: stand-in and real call must both refuse")
    };
    assert_eq!(standin_error, real_error, "{name} {arguments:?}");
    assert_eq!(
        *log.lock().unwrap_or_else(PoisonError::into_inner),
        before_log,
        "a refusal must not append geometry records"
    );
    assert_eq!(
        *tints.lock().unwrap_or_else(PoisonError::into_inner),
        before_tints,
        "a refusal must not append tint records"
    );
}

fn seed_group(scene: &StrictScene) -> String {
    match scene
        .call("create_group", &[])
        .expect("a group is created")
        .as_slice()
    {
        [ConfigValue::String(group)] => group.clone(),
        other => panic!("unexpected group result: {other:?}"),
    }
}

fn seed_quad(scene: &StrictScene, group: Option<&str>) -> (String, String) {
    let texture = scene
        .call("texture.load", &[string("ui/font/glyphs/U+0061.png")])
        .expect("the fixture texture loads");
    let [ConfigValue::String(texture)] = texture.as_slice() else {
        panic!("unexpected texture result: {texture:?}");
    };
    let mut table = BTreeMap::from([
        ("texture".to_owned(), ConfigValue::String(texture.clone())),
        ("x".to_owned(), ConfigValue::Float(1.0)),
        ("y".to_owned(), ConfigValue::Float(2.0)),
        ("width".to_owned(), ConfigValue::Float(3.0)),
        ("height".to_owned(), ConfigValue::Float(4.0)),
    ]);
    if let Some(group) = group {
        table.insert("group".to_owned(), ConfigValue::String(group.to_owned()));
    }
    let quad = scene
        .call("create_quad", &[value(ConfigValue::Table(table))])
        .expect("the fixture quad is created");
    let [ConfigValue::String(quad)] = quad.as_slice() else {
        panic!("unexpected quad result: {quad:?}");
    };
    (quad.clone(), texture.clone())
}

/// Coverage map: arity/type and rule-specific refusals exercise every readout render stand-in and texture.load;
/// the paired calls use the same backing writer/service, with rig aliases translated to their real handles.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one refusal matrix keeps parity cases grouped by the single stand-in contract"
)]
fn every_readout_render_and_texture_standin_matches_the_real_refusal() {
    let (scene, log, tints) = test_strict_scene();
    let group = seed_group(&scene);
    let (quad, texture) = seed_quad(&scene, Some(&group));

    // create_group, create_quad and texture.load: arity, type, geometry, tint, texture and group checks.
    for (name, args) in [
        ("create_group", vec![string("unexpected")]),
        ("create_quad", vec![]),
        ("create_quad", vec![string("bad"), string("extra")]),
        ("create_quad", vec![string("not a table")]),
        (
            "create_quad",
            vec![value(ConfigValue::Table(BTreeMap::from([
                (
                    "texture".to_owned(),
                    ConfigValue::String("t:unknown.png".to_owned()),
                ),
                ("width".to_owned(), ConfigValue::Float(1.0)),
                ("height".to_owned(), ConfigValue::Float(1.0)),
            ])))],
        ),
        (
            "create_quad",
            vec![value(ConfigValue::Table(BTreeMap::from([
                ("texture".to_owned(), ConfigValue::String(texture.clone())),
                ("x".to_owned(), ConfigValue::Float(0.0)),
                ("y".to_owned(), ConfigValue::Float(0.0)),
                ("width".to_owned(), ConfigValue::Float(0.0)),
                ("height".to_owned(), ConfigValue::Float(1.0)),
            ])))],
        ),
        (
            "create_quad",
            vec![value(ConfigValue::Table(BTreeMap::from([
                ("texture".to_owned(), ConfigValue::String(texture.clone())),
                ("width".to_owned(), ConfigValue::Float(1.0)),
                ("height".to_owned(), ConfigValue::Float(1.0)),
                ("tint".to_owned(), ConfigValue::String("red".to_owned())),
            ])))],
        ),
        (
            "create_quad",
            vec![value(ConfigValue::Table(BTreeMap::from([
                ("texture".to_owned(), ConfigValue::String(texture.clone())),
                ("width".to_owned(), ConfigValue::Float(1.0)),
                ("height".to_owned(), ConfigValue::Float(1.0)),
                ("group".to_owned(), ConfigValue::String("g999".to_owned())),
            ])))],
        ),
        ("texture.load", vec![]),
        ("texture.load", vec![value(ConfigValue::Integer(3))]),
        ("texture.load", vec![string("../escape.png")]),
        ("texture.load", vec![string("a.png"), string("extra")]),
    ] {
        assert_refusal_matches_real(&scene, name, &args, &log, &tints);
    }

    // raise_groups: shape, member type, unknown and tombstoned group names.
    let stale_group = seed_group(&scene);
    scene
        .call("destroy_group", &[string(&stale_group)])
        .expect("the group is destroyed");
    for args in [
        vec![],
        vec![string("a"), string("b")],
        vec![value(ConfigValue::Array(vec![ConfigValue::Integer(1)]))],
        vec![value(ConfigValue::Array(vec![ConfigValue::String(
            "g999".to_owned(),
        )]))],
        vec![value(ConfigValue::Array(vec![ConfigValue::String(
            stale_group.clone(),
        )]))],
    ] {
        assert_refusal_matches_real(&scene, "raise_groups", &args, &log, &tints);
    }

    // set_hit: arity, key/rectangle types and values, plus unknown and tombstoned optional groups.
    for args in [
        vec![string("bad")],
        vec![
            string("bad"),
            value(ConfigValue::Array(vec![])),
            string("g1"),
            string("extra"),
        ],
        vec![
            value(ConfigValue::Integer(1)),
            value(ConfigValue::Array(vec![])),
        ],
        vec![string("bad:key"), value(ConfigValue::Array(vec![]))],
        vec![
            string("valid"),
            value(ConfigValue::Array(vec![ConfigValue::Integer(1)])),
        ],
        vec![
            string("valid"),
            value(ConfigValue::Array(vec![rect(0.0, 0.0, 0.0, 1.0)])),
        ],
        vec![
            string("valid"),
            value(ConfigValue::Array(vec![])),
            string("g999"),
        ],
        vec![
            string("valid"),
            value(ConfigValue::Array(vec![])),
            string(&stale_group),
        ],
    ] {
        assert_refusal_matches_real(&scene, "set_hit", &args, &log, &tints);
    }

    // Quad calls: arity/type, real rectangle/tint checks and unknown/destroyed handle checks.
    let stale_quad = seed_quad(&scene, None).0;
    scene
        .call("destroy_quad", &[string(&stale_quad)])
        .expect("the quad is destroyed");
    for (name, args) in [
        ("set_position", vec![string(&quad), number(1.0)]),
        (
            "set_position",
            vec![string(&quad), number(1.0), number(2.0), number(3.0)],
        ),
        (
            "set_position",
            vec![value(ConfigValue::Integer(1)), number(1.0), number(2.0)],
        ),
        (
            "set_position",
            vec![
                string(&quad),
                value(ConfigValue::String("x".to_owned())),
                number(2.0),
            ],
        ),
        (
            "set_position",
            vec![string(&quad), number(f64::INFINITY), number(2.0)],
        ),
        (
            "set_position",
            vec![string("q999"), number(1.0), number(2.0)],
        ),
        (
            "set_position",
            vec![string(&stale_quad), number(1.0), number(2.0)],
        ),
        (
            "set_rect",
            vec![string(&quad), number(1.0), number(2.0), number(3.0)],
        ),
        (
            "set_rect",
            vec![
                string(&quad),
                number(1.0),
                number(2.0),
                number(3.0),
                number(4.0),
                number(5.0),
            ],
        ),
        (
            "set_rect",
            vec![
                string(&quad),
                string("x"),
                number(2.0),
                number(3.0),
                number(4.0),
            ],
        ),
        (
            "set_rect",
            vec![
                string(&quad),
                number(1.0),
                number(2.0),
                number(0.0),
                number(4.0),
            ],
        ),
        (
            "set_rect",
            vec![
                string("q999"),
                number(1.0),
                number(2.0),
                number(3.0),
                number(4.0),
            ],
        ),
        (
            "set_rect",
            vec![
                string(&stale_quad),
                number(1.0),
                number(2.0),
                number(3.0),
                number(4.0),
            ],
        ),
        ("set_tint", vec![string(&quad)]),
        (
            "set_tint",
            vec![string(&quad), string("red"), string("extra")],
        ),
        (
            "set_tint",
            vec![value(ConfigValue::Integer(1)), string("#ffffff")],
        ),
        ("set_tint", vec![string(&quad), string("#fffff")]),
        ("set_tint", vec![string("q999"), string("#ffffff")]),
        ("set_tint", vec![string(&stale_quad), string("#ffffff")]),
        ("destroy_quad", vec![]),
        ("destroy_quad", vec![string(&quad), string("extra")]),
        ("destroy_quad", vec![value(ConfigValue::Integer(1))]),
        ("destroy_quad", vec![string("q999")]),
        ("destroy_quad", vec![string(&stale_quad)]),
    ] {
        assert_refusal_matches_real(&scene, name, &args, &log, &tints);
    }

    // Group calls: arity/type, clips/number/bool validation, and unknown/destroyed handles.
    let live_group = seed_group(&scene);
    for (name, args) in [
        ("set_group_clip", vec![string(&live_group)]),
        (
            "set_group_clip",
            vec![
                string(&live_group),
                value(rect(0.0, 0.0, -1.0, 2.0)),
                string("extra"),
            ],
        ),
        (
            "set_group_clip",
            vec![
                value(ConfigValue::Integer(1)),
                value(rect(0.0, 0.0, 1.0, 1.0)),
            ],
        ),
        (
            "set_group_clip",
            vec![string(&live_group), string("not a clip")],
        ),
        (
            "set_group_clip",
            vec![string(&live_group), value(rect(0.0, 0.0, -1.0, 2.0))],
        ),
        (
            "set_group_clip",
            vec![string("g999"), value(rect(0.0, 0.0, 1.0, 1.0))],
        ),
        (
            "set_group_clip",
            vec![string(&stale_group), value(rect(0.0, 0.0, 1.0, 1.0))],
        ),
        ("set_group_offset", vec![string(&live_group), number(1.0)]),
        (
            "set_group_offset",
            vec![string(&live_group), number(1.0), number(2.0), number(3.0)],
        ),
        (
            "set_group_offset",
            vec![
                string(&live_group),
                value(ConfigValue::String("x".to_owned())),
                number(2.0),
            ],
        ),
        (
            "set_group_offset",
            vec![string(&live_group), number(f64::INFINITY), number(2.0)],
        ),
        (
            "set_group_offset",
            vec![string("g999"), number(1.0), number(2.0)],
        ),
        (
            "set_group_offset",
            vec![string(&stale_group), number(1.0), number(2.0)],
        ),
        ("set_group_visible", vec![string(&live_group)]),
        (
            "set_group_visible",
            vec![
                string(&live_group),
                value(ConfigValue::Bool(true)),
                string("extra"),
            ],
        ),
        (
            "set_group_visible",
            vec![string(&live_group), string("yes")],
        ),
        (
            "set_group_visible",
            vec![string("g999"), value(ConfigValue::Bool(true))],
        ),
        (
            "set_group_visible",
            vec![string(&stale_group), value(ConfigValue::Bool(true))],
        ),
        ("destroy_group", vec![]),
        ("destroy_group", vec![string(&live_group), string("extra")]),
        ("destroy_group", vec![value(ConfigValue::Integer(1))]),
        ("destroy_group", vec![string("g999")]),
        ("destroy_group", vec![string(&stale_group)]),
    ] {
        assert_refusal_matches_real(&scene, name, &args, &log, &tints);
    }
}

#[test]
fn strict_readout_standins_keep_the_accepted_call_records() {
    let (scene, log, tints) = test_strict_scene();
    let group = seed_group(&scene);
    let (quad, _texture) = seed_quad(&scene, Some(&group));
    scene
        .call(
            "raise_groups",
            &[value(ConfigValue::Array(vec![ConfigValue::String(
                group.clone(),
            )]))],
        )
        .expect("a known group can be raised");
    scene
        .call(
            "set_hit",
            &[
                string("panel"),
                value(ConfigValue::Array(vec![rect(1.0, 2.0, 3.0, 4.0)])),
                string(&group),
            ],
        )
        .expect("a valid hit region is recorded");
    scene
        .call("set_position", &[string(&quad), number(5.0), number(6.0)])
        .expect("position is accepted");
    scene
        .call(
            "set_rect",
            &[
                string(&quad),
                number(1.0),
                number(2.0),
                number(3.0),
                number(4.0),
            ],
        )
        .expect("rect is accepted");
    scene
        .call("set_tint", &[string(&quad), string("#abcdef")])
        .expect("tint is accepted");
    scene
        .call(
            "set_group_clip",
            &[string(&group), value(rect(0.0, 0.0, 10.0, 10.0))],
        )
        .expect("clip is accepted");
    scene
        .call(
            "set_group_offset",
            &[string(&group), number(7.0), number(8.0)],
        )
        .expect("offset is accepted");
    scene
        .call(
            "set_group_visible",
            &[string(&group), value(ConfigValue::Bool(false))],
        )
        .expect("visibility is accepted");
    scene
        .call("destroy_quad", &[string(&quad)])
        .expect("quad is destroyed");
    scene
        .call("destroy_group", &[string(&group)])
        .expect("group is destroyed");
    assert_eq!(
        *log.lock().unwrap_or_else(PoisonError::into_inner),
        [
            "create_group g1",
            "load ui/font/glyphs/U+0061.png",
            "create q1 ui/font/glyphs/U+0061.png 1 2 3 4",
            "set_hit panel 1 2 3 4",
            "set_position q1 5 6",
            "set_rect q1 1 2 3 4",
            "set_group_clip g1 Table({\"height\": Float(10.0), \"width\": Float(10.0), \"x\": Float(0.0), \"y\": Float(0.0)})",
            "set_group_offset g1 7 8",
            "set_group_visible g1 false",
            "destroy q1",
            "destroy_group g1",
        ]
    );
    assert_eq!(
        *tints.lock().unwrap_or_else(PoisonError::into_inner),
        ["set_tint q1 #abcdef"]
    );
}

fn translate(
    name: &str,
    arguments: &[Option<ConfigValue>],
    names: &Arc<Mutex<Names>>,
) -> Vec<Option<ConfigValue>> {
    let names = names.lock().unwrap_or_else(PoisonError::into_inner);
    let map = |value: &ConfigValue, kind: &str| {
        let ConfigValue::String(alias) = value else {
            return value.clone();
        };
        let real = match kind {
            "quad" => names.quads.get(alias).unwrap_or(&names.foreign_quad),
            "group" => names.groups.get(alias).unwrap_or(&names.foreign_group),
            "texture" => names.textures.get(alias).unwrap_or(&names.foreign_texture),
            _ => unreachable!("known handle kind"),
        };
        ConfigValue::String(real.clone())
    };
    let mut values = arguments.to_vec();
    let mut at = |index: usize, kind: &str| {
        if let Some(Some(value)) = values.get_mut(index) {
            *value = map(value, kind);
        }
    };
    match name {
        "create_quad" => {
            if let Some(Some(ConfigValue::Table(table))) = values.first_mut() {
                for key in ["texture", "group"] {
                    if let Some(value) = table.get(key) {
                        table.insert(
                            key.to_owned(),
                            map(value, if key == "texture" { "texture" } else { "group" }),
                        );
                    }
                }
            }
        }
        "raise_groups" => {
            if let Some(Some(ConfigValue::Array(items))) = values.first_mut() {
                for item in items {
                    *item = map(item, "group");
                }
            }
        }
        "set_hit" => at(2, "group"),
        "set_position" | "set_rect" | "set_tint" | "destroy_quad" => at(0, "quad"),
        "set_group_clip" | "set_group_offset" | "set_group_visible" | "destroy_group" => {
            at(0, "group");
        }
        _ => {}
    }
    values
}

fn record_success(
    name: &str,
    arguments: &[Option<ConfigValue>],
    result: &[ConfigValue],
    names: &Arc<Mutex<Names>>,
    log: &Log,
    tints: &Log,
) -> Result<Vec<ConfigValue>, String> {
    match name {
        "create_group" => {
            let Some(ConfigValue::String(real)) = result.first() else {
                return Err("render.create_group: real handler returned no token".to_owned());
            };
            let alias = {
                let mut names = names.lock().unwrap_or_else(PoisonError::into_inner);
                names.group_count += 1;
                let alias = format!("g{}", names.group_count);
                names.groups.insert(alias.clone(), real.clone());
                alias
            };
            push(log, format!("create_group {alias}"));
            Ok(vec![ConfigValue::String(alias)])
        }
        "create_quad" => {
            let Some(ConfigValue::String(real)) = result.first() else {
                return Err("render.create_quad: real handler returned no token".to_owned());
            };
            let alias = {
                let mut names = names.lock().unwrap_or_else(PoisonError::into_inner);
                names.quad_count += 1;
                let alias = format!("q{}", names.quad_count);
                names.quads.insert(alias.clone(), real.clone());
                alias
            };
            if let Some(Some(table)) = arguments.first() {
                let texture = field(table, "texture");
                push(
                    log,
                    format!(
                        "create {alias} {} {} {} {} {}",
                        texture.trim_start_matches("t:"),
                        field(table, "x"),
                        field(table, "y"),
                        field(table, "width"),
                        field(table, "height")
                    ),
                );
                let tint = field(table, "tint");
                if !tint.is_empty() {
                    push(tints, format!("create {alias} {tint}"));
                }
            }
            Ok(vec![ConfigValue::String(alias)])
        }
        "raise_groups" => Ok(result.to_vec()),
        "set_hit" => {
            if let Some(Some(ConfigValue::String(key))) = arguments.first() {
                match arguments.get(1).and_then(Option::as_ref) {
                    Some(ConfigValue::Array(rects)) => {
                        for rect in rects {
                            push(
                                log,
                                format!(
                                    "set_hit {key} {} {} {} {}",
                                    field(rect, "x"),
                                    field(rect, "y"),
                                    field(rect, "width"),
                                    field(rect, "height")
                                ),
                            );
                        }
                    }
                    _ => push(log, format!("set_hit {key} nil")),
                }
            }
            Ok(result.to_vec())
        }
        other => {
            let target = if other == "set_tint" { tints } else { log };
            let rendered: Vec<String> = arguments.iter().flatten().map(show).collect();
            let label = if other == "destroy_quad" {
                "destroy"
            } else {
                other
            };
            push(
                target,
                format!("{label} {}", rendered.join(" "))
                    .trim_end()
                    .to_owned(),
            );
            Ok(result.to_vec())
        }
    }
}

fn record_load(
    arguments: &[Option<ConfigValue>],
    result: &[ConfigValue],
    names: &Arc<Mutex<Names>>,
    log: &Log,
) -> Vec<ConfigValue> {
    let (Some(Some(ConfigValue::String(name))), Some(ConfigValue::String(real))) =
        (arguments.first(), result.first())
    else {
        return result.to_vec();
    };
    let alias = format!("t:{name}");
    names
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .textures
        .insert(alias.clone(), real.clone());
    push(log, format!("load {name}"));
    vec![ConfigValue::String(alias)]
}

fn observed_scene(
    log: &Log,
    tints: &Log,
    assets: &std::path::Path,
    observer: SceneCalls,
) -> Vec<Extension> {
    Arc::new(StrictScene::new(log, tints, assets, observer)).extensions()
}

/// The shipped UI scripts' directory: the root of the UI VM's `require`.
fn ui_scripts() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui")
}

/// A UI VM over a recording scene with the real pixel font, the `game` and `frame` extensions and `require` rooted at the
/// shipped `scripts/ui`: the script is the shipped `main.lua` followed by a tail (`new`), or just a script (`script`).
pub(crate) struct Readout {
    owner: VmOwner,
    log: Log,
    tints: Log,
    scene_calls: SceneCalls,
    registered: Arc<AtomicBool>,
    hover_registered: Arc<AtomicBool>,
    frame: FrameRegistration,
    controls: std::sync::mpsc::Receiver<Message>,
    sequence: u32,
    _dir: Dir,
}

impl Readout {
    /// The UI script (`scripts/ui/main.lua`, as shipped) followed by `tail`.
    pub(crate) fn new(tail: &str) -> Self {
        let shipped =
            fs::read_to_string(ui_scripts().join("main.lua")).expect("the shipped UI script");
        Self::script(&format!("{shipped}\n{tail}\n"))
    }

    /// `script` as the UI script.
    pub(crate) fn script(script: &str) -> Self {
        Self::with_options(script, TooltipsConfig::default(), None)
    }

    pub(crate) fn script_with_tooltips(script: &str, tooltips: TooltipsConfig) -> Self {
        Self::with_options(script, tooltips, None)
    }

    pub(crate) fn script_with_tooltips_and_initial_serial(
        script: &str,
        tooltips: TooltipsConfig,
        initial_serial: u64,
    ) -> Self {
        Self::with_initial_serial(script, tooltips, initial_serial)
    }

    pub(crate) fn script_with_glossary(script: &str, glossary: &str) -> Self {
        Self::with_options(script, TooltipsConfig::default(), Some(glossary))
    }

    pub(crate) fn script_with_glossary_and_tooltips(
        script: &str,
        glossary: &str,
        tooltips: TooltipsConfig,
    ) -> Self {
        Self::with_options(script, tooltips, Some(glossary))
    }

    fn with_options(script: &str, tooltips: TooltipsConfig, glossary_text: Option<&str>) -> Self {
        Self::with_optional_serial(script, tooltips, glossary_text, None)
    }

    fn with_initial_serial(script: &str, tooltips: TooltipsConfig, initial_serial: u64) -> Self {
        Self::with_optional_serial(script, tooltips, None, Some(initial_serial))
    }

    fn with_optional_serial(
        script: &str,
        tooltips: TooltipsConfig,
        glossary_text: Option<&str>,
        initial_serial: Option<u64>,
    ) -> Self {
        let dir = Dir::new();
        let entry = dir.0.join("main.lua");
        fs::write(&entry, script).expect("the script");
        let modules_root = if let Some(initial_serial) = initial_serial {
            let root = dir.0.join("modules");
            Self::copy_directory(&ui_scripts(), &root);
            let tooltip = root.join("lib/tooltip.lua");
            let source = fs::read_to_string(&tooltip).expect("the tooltip library");
            let source = source.replacen(
                "local serial = 0",
                &format!("local serial = {initial_serial}"),
                1,
            );
            fs::write(tooltip, source).expect("the test tooltip library");
            root
        } else {
            ui_scripts()
        };
        let log = Log::default();
        let tints = Log::default();
        let scene_calls = SceneCalls::default();
        let notifier = Notifier::new();
        let requests = Requests::new();
        let mut window = window_api::extension(requests.writer(Arc::clone(&notifier)), None, None);
        // A record goes to both logs, so that the tints can be read between the script's marks too.
        let (record_geometry, record_tints) =
            (recording(&log, "record"), recording(&tints, "record"));
        window = window.function(
            "record",
            Arc::new(move |arguments| {
                record_tints(arguments)?;
                record_geometry(arguments)
            }),
        );
        let registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let (control_sender, controls) = std::sync::mpsc::channel();
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let shipped_font = Arc::new(font::load_all(&assets).expect("the shipped fonts"));
        let config = ConfigValue::Table(BTreeMap::new());
        let frame = FrameRegistration::default();
        let mut settings = VmSettings::new("ui", entry, config);
        settings.modules = Modules::Rooted(modules_root);
        settings.extensions = vec![window];
        settings.extensions.extend(observed_scene(
            &log,
            &tints,
            &assets,
            Arc::clone(&scene_calls),
        ));
        settings
            .extensions
            .push(font::extension(Arc::clone(&shipped_font)));
        settings.extensions.push(crate::theme::extension(Arc::new(
            crate::theme::load(&assets).expect("the shipped theme"),
        )));
        settings.extensions.push(frame_api::extension(
            false,
            FrameBackend::Headless,
            frame.clone(),
        ));
        settings.extensions.push(game_api::extension(
            Arc::clone(&registered),
            Arc::clone(&hover_registered),
            Arc::new(|| {}),
            crate::game::controls::SimControls::new(control_sender),
        ));
        settings.extensions.push(tooltips.extension());
        let glossary = match glossary_text {
            Some(text) => crate::glossary::parse(text, &shipped_font).expect("fixture glossary"),
            None => crate::glossary::load(&assets, &shipped_font).expect("the shipped glossary"),
        };
        settings
            .extensions
            .push(crate::glossary::extension(Arc::new(glossary)));
        settings.call_limit = 4;
        let mut owner = VmOwner::spawn(settings).expect("the UI VM spawns");
        owner.wait_ready(Duration::from_secs(5)).expect("ready");
        owner.start(Duration::from_secs(5)).expect("started");
        Self {
            owner,
            log,
            tints,
            scene_calls,
            registered,
            hover_registered,
            frame,
            controls,
            sequence: 0,
            _dir: dir,
        }
    }

    fn copy_directory(source: &std::path::Path, destination: &std::path::Path) {
        fs::create_dir_all(destination).expect("the test module directory");
        for entry in fs::read_dir(source).expect("the shipped UI scripts") {
            let entry = entry.expect("a UI script entry");
            let target = destination.join(entry.file_name());
            if entry.file_type().expect("the UI script type").is_dir() {
                Self::copy_directory(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).expect("the UI script copy");
            }
        }
    }

    /// Whether the script has a `frame_update` callback registered.
    pub(crate) fn frame_registered(&self) -> bool {
        self.frame.state().1
    }

    /// The tints, in order: `create <quad> <colour>` for a quad created with one, and `set_tint <quad> <colour>`.
    pub(crate) fn tints(&self) -> Vec<String> {
        self.tints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn entries(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn observed_calls(&self) -> Vec<ObservedHostCall> {
        self.scene_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn clear_observed_calls(&self) {
        self.scene_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    pub(crate) fn controls(&self) -> Vec<crate::game::controls::Control> {
        self.controls
            .try_iter()
            .filter_map(|message| match message {
                Message::Control(control) => Some(control),
                _ => None,
            })
            .collect()
    }

    /// The entries after the `n`th `record mark` entry, up to the next one.
    pub(crate) fn after_mark(&self, n: usize) -> Vec<String> {
        let entries = self.entries();
        let marks: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.as_str() == "record mark")
            .map(|(index, _)| index)
            .collect();
        let start = marks[n - 1] + 1;
        let end = marks.get(n).copied().unwrap_or(entries.len());
        entries[start..end].to_vec()
    }

    /// The tints after the `n`th `record mark`, up to the next one (see [`tints`](Self::tints)).
    pub(crate) fn tints_after_mark(&self, n: usize) -> Vec<String> {
        let entries = self.tints();
        let marks: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.as_str() == "record mark")
            .map(|(index, _)| index)
            .collect();
        let start = marks[n - 1] + 1;
        let end = marks.get(n).copied().unwrap_or(entries.len());
        entries[start..end].to_vec()
    }

    /// Posts `game_time` with `snapshot` and waits for the call to end.
    pub(crate) fn deliver(&mut self, snapshot: &SimSnapshot) {
        let id = self
            .owner
            .post(game_api::GAME_TIME, vec![game_api::time_table(snapshot)])
            .expect("posted");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = self.owner.take_result() {
                assert_eq!(result.id, id);
                assert!(
                    matches!(result.outcome, CallOutcome::Returned(_)),
                    "{:?}",
                    result.outcome
                );
                return;
            }
            assert!(Instant::now() < deadline, "the callback did not return");
            std::thread::yield_now();
        }
    }

    fn deliver_select(&mut self, id: &str, name: &str, kind: &str, size: u8) {
        let arguments = game_api::select_arguments(&crate::map_app::Selection {
            seq: 1,
            id: id.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            size,
        });
        let id = self
            .owner
            .post(game_api::GAME_SELECT, arguments)
            .expect("posted");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = self.owner.take_result() {
                assert_eq!(result.id, id);
                assert!(
                    matches!(result.outcome, CallOutcome::Returned(_)),
                    "{:?}",
                    result.outcome
                );
                return;
            }
            assert!(Instant::now() < deadline, "the callback did not return");
            std::thread::yield_now();
        }
    }

    pub(crate) fn deliver_hover(&mut self, hover: Option<(&str, &str, i64, i64, i64)>) {
        let arguments = hover.map_or_else(Vec::new, |(id, name, left, right, y)| {
            vec![ConfigValue::Table(BTreeMap::from([
                ("id".to_owned(), ConfigValue::String(id.to_owned())),
                ("name".to_owned(), ConfigValue::String(name.to_owned())),
                ("left".to_owned(), ConfigValue::Integer(left)),
                ("right".to_owned(), ConfigValue::Integer(right)),
                ("y".to_owned(), ConfigValue::Integer(y)),
            ]))]
        });
        let id = self
            .owner
            .post(game_api::GAME_HOVER, arguments)
            .expect("posted");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = self.owner.take_result() {
                assert_eq!(result.id, id);
                assert!(
                    matches!(result.outcome, CallOutcome::Returned(_)),
                    "{:?}",
                    result.outcome
                );
                return;
            }
            assert!(Instant::now() < deadline, "the callback did not return");
            std::thread::yield_now();
        }
    }

    /// Posts a `frame_update` for a window of `width` by `height` points (with `pointer`, if any) and waits for the call to end.
    pub(crate) fn update(&mut self, width: i64, height: i64, pointer: Option<ConfigValue>) {
        self.sequence += 1;
        let mut fields = BTreeMap::from([
            ("dt".to_owned(), ConfigValue::Float(0.016)),
            ("elapsed".to_owned(), ConfigValue::Float(0.016)),
            (
                "sequence".to_owned(),
                ConfigValue::Integer(i64::from(self.sequence)),
            ),
            ("width".to_owned(), ConfigValue::Integer(width)),
            ("height".to_owned(), ConfigValue::Integer(height)),
            ("actions".to_owned(), ConfigValue::Table(BTreeMap::new())),
            ("transitions".to_owned(), ConfigValue::Array(Vec::new())),
            ("overflow".to_owned(), ConfigValue::Bool(false)),
            ("reset".to_owned(), ConfigValue::Bool(false)),
        ]);
        if let Some(pointer) = pointer {
            fields.insert("pointer".to_owned(), pointer);
        }
        let id = self
            .owner
            .post(FRAME_UPDATE, vec![ConfigValue::Table(fields)])
            .expect("posted");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = self.owner.take_result() {
                assert_eq!(result.id, id);
                assert!(
                    matches!(result.outcome, CallOutcome::Returned(_)),
                    "{:?}",
                    result.outcome
                );
                return;
            }
            assert!(Instant::now() < deadline, "the callback did not return");
            std::thread::yield_now();
        }
    }
}

impl Drop for Readout {
    fn drop(&mut self) {
        self.owner.cancel();
        let _ = self.owner.wait_finished(Duration::from_secs(5));
    }
}

pub(crate) fn snapshot(hour: u8, speed: u8, paused: Option<PauseReason>) -> SimSnapshot {
    let date = ck_sim::Date {
        year: 768,
        month: 10,
        day: 9,
        hour,
    };
    SimSnapshot {
        tick: ck_sim::GameTime::from_date(date).expect("a valid date"),
        date,
        speed: ck_sim::Speed::new(speed).expect("a speed"),
        paused,
        economy_revision: 0,
        economy: None,
    }
}

#[test]
fn setting_the_next_hour_replaces_one_quad_and_moves_none() {
    let readout = Readout::new(
        "local l = text.line(10, 20)\ntext.set(l, '14:00')\nwindow.record('mark')\ntext.set(l, '15:00')\nwindow.record('mark')",
    );
    // The first set: one quad for each of the five glyphs, at 2 points per font pixel (the colon is narrow, so the zeros come
    // at 14 and 20 font pixels).
    let first: Vec<String> = readout
        .entries()
        .into_iter()
        .take_while(|entry| entry != "record mark")
        .collect();
    assert_eq!(
        first
            .iter()
            .filter(|entry| entry.starts_with("create"))
            .count(),
        5,
        "{first:?}"
    );
    assert_eq!(first[0], "load ui/font/glyphs/U+0031.png");
    assert!(
        first.contains(&"create q1 ui/font/glyphs/U+0031.png 10 20 10 22".to_owned()),
        "{first:?}"
    );
    assert!(
        first.contains(&"create q4 ui/font/glyphs/U+0030.png 38 20 10 22".to_owned()),
        "{first:?}"
    );
    // The next hour changes one digit: that quad is replaced, and no other is touched.
    assert_eq!(
        readout.after_mark(1),
        [
            "destroy q2",
            "load ui/font/glyphs/U+0035.png",
            "create q6 ui/font/glyphs/U+0035.png 22 20 10 22",
        ]
    );
}

#[test]
fn a_wider_glyph_moves_the_later_quads_and_replaces_nothing_else() {
    let readout = Readout::new(
        "local l = text.line(0, 0)\ntext.set(l, 'iab')\nwindow.record('mark')\ntext.set(l, 'mab')\nwindow.record('mark')",
    );
    assert_eq!(
        readout.after_mark(1),
        [
            "destroy q1",
            "load ui/font/glyphs/U+006D.png",
            "create q4 ui/font/glyphs/U+006D.png 0 0 10 22",
            "set_position q2 12 0",
            "set_position q3 24 0",
        ]
    );
}

#[test]
fn an_unchanged_string_makes_no_scene_call_and_a_shorter_one_destroys_the_tail() {
    let readout = Readout::new(
        "local l = text.line(0, 0)\ntext.set(l, 'abc')\nwindow.record('mark')\ntext.set(l, 'abc')\nwindow.record('mark')\ntext.set(l, 'a')\nwindow.record('mark')\ntext.set(l, 'a')\nwindow.record('mark')",
    );
    assert!(
        readout.after_mark(1).is_empty(),
        "{:?}",
        readout.after_mark(1)
    );
    assert_eq!(readout.after_mark(2), ["destroy q2", "destroy q3"]);
    assert!(readout.after_mark(3).is_empty());
}

#[test]
fn move_repositions_every_quad_once_and_the_same_position_makes_no_call() {
    let readout = Readout::new(
        "local l = text.line(0, 0)\ntext.set(l, 'ab')\nwindow.record('mark')\ntext.move(l, 6, 8)\nwindow.record('mark')\ntext.move(l, 6, 8)\nwindow.record('mark')\ntext.destroy(l)\nwindow.record('mark')",
    );
    assert_eq!(
        readout.after_mark(1),
        ["set_position q1 6 8", "set_position q2 18 8"]
    );
    assert!(readout.after_mark(2).is_empty());
    assert_eq!(readout.after_mark(3), ["destroy q1", "destroy q2"]);
}

#[test]
fn the_font_is_drawn_at_two_units_per_pixel() {
    let readout = Readout::new("local l = text.line(0, 0)\ntext.set(l, 'a')");
    assert!(
        readout
            .entries()
            .contains(&"create q1 ui/font/glyphs/U+0061.png 0 0 10 22".to_owned()),
        "{:?}",
        readout.entries()
    );
}

#[test]
fn ui_group_seam_keeps_the_registered_group_last_and_observes_exact_host_calls() {
    let readout = Readout::script(
        r#"
local ui = require("lib.ui")
local a = render.create_group()
local top = render.create_group()
local b = render.create_group()
ui.set_top_group(top)
ui.raise_groups({a, b})
ui.raise_groups({a, top, b})
ui.raise_groups({top, a})
ui.set_top_group(nil)
ui.raise_groups({a, b})
ui.set_top_group(top)
ui.create_group()
"#,
    );
    let calls = readout.observed_calls();
    let raises: Vec<Vec<String>> = calls
        .iter()
        .filter(|call| call.name == "render.raise_groups")
        .map(|call| {
            let [Some(ConfigValue::Array(groups))] = call.arguments.as_slice() else {
                panic!("raise_groups receives one group array: {call:?}");
            };
            groups
                .iter()
                .map(|group| match group {
                    ConfigValue::String(group) => group.clone(),
                    other => panic!("group alias: {other:?}"),
                })
                .collect()
        })
        .collect();
    assert_eq!(
        raises,
        vec![
            vec!["g1".to_owned(), "g3".to_owned(), "g2".to_owned()],
            vec!["g1".to_owned(), "g3".to_owned(), "g2".to_owned()],
            vec!["g1".to_owned(), "g2".to_owned()],
            vec!["g1".to_owned(), "g3".to_owned()],
            vec!["g2".to_owned()],
        ]
    );
    let create_positions: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.name == "render.create_group")
        .map(|(index, _)| index)
        .collect();
    let last_raise = calls
        .iter()
        .rposition(|call| call.name == "render.raise_groups")
        .expect("ui.create_group raises the registered overlay");
    assert_eq!(
        calls[last_raise].arguments,
        [Some(ConfigValue::Array(vec![ConfigValue::String(
            "g2".to_owned()
        ),]))]
    );
    assert_eq!(create_positions.len(), 4);
    assert!(create_positions[3] < last_raise);
    assert_eq!(calls[last_raise].name, "render.raise_groups");
}

#[test]
fn the_readout_format() {
    let readout = Readout::new(
        "local date = require('lib.time').date\n\
         window.record(date({year = 768, month = 10, day = 9, hour = 14}))\n\
         window.record(date({year = 768, month = 10, day = 9, hour = 14}))\n\
         window.record(date({year = 1, month = 1, day = 1, hour = 5}))\n\
         window.record(date({year = 9999, month = 12, day = 31, hour = 23}))",
    );
    let records: Vec<String> = readout
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("record "))
        .collect();
    assert_eq!(
        records,
        [
            "record 9 Oct 768 · 14:00",
            "record 9 Oct 768 · 14:00",
            "record 1 Jan 1 · 05:00",
            "record 31 Dec 9999 · 23:00",
        ]
    );
}

#[test]
fn the_time_bar_creates_its_panel_first_then_touches_only_what_changed() {
    let mut readout = Readout::new("");
    assert!(
        readout.registered.load(Ordering::SeqCst),
        "the script registered game.on_time"
    );
    readout.deliver(&snapshot(14, 1, Some(PauseReason::Load)));
    let first = readout.entries();
    // The cut-6 panel is at the snapped margin with fixed transport controls, boxes, Help and a date-only readout.
    assert_eq!(first[0], "load ui/theme/cut6_tl_fill.png");
    assert!(
        first[1].starts_with("create q1 ui/theme/cut6_tl_fill.png 8 8 "),
        "{first:?}"
    );
    // Three panels (the time bar, the ledger bar and the map modes' bar) and nine buttons use cut-6 frames; icons are precreated
    // (including both pause/play states, the two ledger tabs, and the three map modes'), and the five boxes have separate outline and fill quads.
    let text = "9 Oct 768 · 14:00";
    let glyphs = text.chars().filter(|c| *c != ' ').count();
    assert_eq!(
        first.iter().filter(|e| e.starts_with("create ")).count(),
        glyphs + 3 * 11 + 8 * 11 + 7 + 1 + 6 + 10 + 7,
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .any(|entry| entry.starts_with("set_hit time_bar 8 8 ")),
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .any(|entry| entry.starts_with("set_hit places_bar "))
    );

    // Only the hour changes: one digit is replaced, the panel keeps its size.
    let before = readout.entries().len();
    readout.deliver(&snapshot(15, 1, Some(PauseReason::Load)));
    let second = readout.entries()[before..].to_vec();
    assert_eq!(
        second.iter().filter(|e| e.starts_with("destroy")).count(),
        1,
        "{second:?}"
    );
    assert_eq!(
        second.iter().filter(|e| e.starts_with("create")).count(),
        1,
        "{second:?}"
    );
    assert!(
        !second.iter().any(|e| e.starts_with("set_rect")),
        "{second:?}"
    );

    // Speed changes only the two inner boxes whose fill state changes; the date and panel width are independent of speed.
    let before = readout.entries().len();
    let before_tints = readout.tints().len();
    readout.deliver(&snapshot(15, 3, None));
    let third = readout.entries()[before..].to_vec();
    let tint_calls = &readout.tints()[before_tints..];
    assert_eq!(tint_calls.len(), 4, "{tint_calls:?}");
    assert!(
        !third.iter().any(|entry| entry.starts_with("set_rect")),
        "{third:?}"
    );
}

#[test]
fn place_tooltip_waits_for_its_delay_then_closes_when_the_place_is_left() {
    let mut readout = Readout::new(PLACE_TOOLTIP_TEST_FIXTURE);
    assert!(readout.hover_registered.load(Ordering::SeqCst));
    let panel_count = |readout: &Readout| {
        let mut panels = std::collections::BTreeSet::new();
        for line in readout.entries() {
            let fields: Vec<_> = line.split_whitespace().collect();
            match fields.as_slice() {
                ["create", name, "ui/theme/cut3_tl_fill.png", ..] => {
                    panels.insert((*name).to_owned());
                }
                ["destroy", name] => {
                    panels.remove(*name);
                }
                _ => {}
            }
        }
        panels.len()
    };
    // The shipped script opens a window of 1280 by 800 pixels, which this rig (it has no screen snapshot) resolves one to one; the
    // flipping below is written for a window 800 points wide, so the first update says so.
    readout.update(800, 600, None);

    let initial_panels = panel_count(&readout);
    readout.deliver_hover(Some(("town", "Alpha", 10, 20, 40)));
    assert!(
        panel_count(&readout) == initial_panels,
        "the place tooltip waits for its delay"
    );
    for _ in 0..80 {
        readout.update(800, 600, None);
    }
    let shown = readout.entries();
    assert_eq!(panel_count(&readout), initial_panels + 1, "{shown:?}");
    assert!(readout.tints().iter().any(|line| line.contains("#c9932e")));

    readout.deliver_hover(None);
    readout.update(800, 600, None);
    let hidden = readout.entries();
    assert_eq!(panel_count(&readout), initial_panels, "{hidden:?}");
}

#[test]
fn tooltip_open_is_quiet_and_game_callbacks_restore_the_overlay_in_the_same_call() {
    let mut readout = Readout::new(TWO_PLACE_TOOLTIP_TEST_FIXTURE);
    readout.update(800, 600, None);
    readout.clear_observed_calls();
    readout.deliver_hover(Some(("town", "Alpha", 10, 20, 40)));
    for _ in 0..40 {
        readout.update(800, 600, None);
    }
    let opening = readout.observed_calls();
    assert!(
        !opening
            .iter()
            .any(|call| call.name == "render.raise_groups"),
        "opening the first tooltip does not raise it: {opening:?}"
    );
    let overlay = opening
        .iter()
        .filter(|call| call.name == "render.create_group")
        .filter_map(|call| match call.result.as_slice() {
            [ConfigValue::String(group)] => Some(group.clone()),
            _ => None,
        })
        .next_back()
        .expect("the first tooltip creates its overlay");

    // The configured delay has passed; let the timer lock finish, then unchanged callbacks must be idle.
    for _ in 0..80 {
        readout.update(800, 600, None);
    }
    readout.clear_observed_calls();
    for _ in 0..10 {
        readout.update(800, 600, None);
    }
    assert!(
        readout.observed_calls().is_empty(),
        "ten updates with the locked tooltip and pointer unchanged are idle: {:?}",
        readout.observed_calls()
    );

    // A later map target opens a second tooltip on the persistent overlay without a raise either.
    readout.clear_observed_calls();
    readout.deliver_hover(Some(("beta", "Beta", 40, 50, 60)));
    for _ in 0..40 {
        readout.update(800, 600, None);
    }
    let later_open = readout.observed_calls();
    assert!(
        !later_open
            .iter()
            .any(|call| call.name == "render.raise_groups"),
        "opening a later tooltip does not raise it: {later_open:?}"
    );

    // The initial game_time callback creates the time-bar groups after the tooltip has registered its overlay.
    readout.clear_observed_calls();
    readout.deliver(&snapshot(0, 1, None));
    assert_overlay_is_last_group_order(&readout.observed_calls(), &overlay);

    // Help opens a new window from frame_update, then raises that same window from a later frame_update.
    readout.clear_observed_calls();
    readout.update(800, 600, Some(readout_button_click("button_4")));
    assert_overlay_is_last_group_order(&readout.observed_calls(), &overlay);
    readout.clear_observed_calls();
    readout.update(800, 600, Some(readout_button_click("button_4")));
    assert_overlay_is_last_group_order(&readout.observed_calls(), &overlay);

    // The places-bar button opens the ledger; its second row is the fixture's one location.
    readout.clear_observed_calls();
    readout.update(800, 600, Some(readout_button_click("button_5")));
    assert_overlay_is_last_group_order(&readout.observed_calls(), &overlay);
    let row = readout
        .observed_calls()
        .iter()
        .filter(|call| call.name == "render.set_hit")
        .filter_map(|call| match call.arguments.first() {
            Some(Some(ConfigValue::String(key))) if key.ends_with("_row_2") => Some(key.clone()),
            _ => None,
        })
        .next_back()
        .expect("the ledger publishes its location row key");
    readout.clear_observed_calls();
    readout.update(800, 600, Some(readout_button_click(&row)));
    assert_overlay_is_last_group_order(&readout.observed_calls(), &overlay);

    // A real game_select callback shows the same place after the ledger-row selection.
    readout.clear_observed_calls();
    readout.deliver_select("town", "Alpha", "town", 1);
    assert_overlay_is_last_group_order(&readout.observed_calls(), &overlay);
}

fn readout_button_click(key: &str) -> ConfigValue {
    let event = |kind: &str| {
        ConfigValue::Table(BTreeMap::from([
            ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
            ("key".to_owned(), ConfigValue::String(key.to_owned())),
            ("x".to_owned(), ConfigValue::Integer(300)),
            ("y".to_owned(), ConfigValue::Integer(100)),
            ("inside".to_owned(), ConfigValue::Bool(true)),
        ]))
    };
    ConfigValue::Table(BTreeMap::from([
        ("x".to_owned(), ConfigValue::Integer(300)),
        ("y".to_owned(), ConfigValue::Integer(100)),
        ("held".to_owned(), ConfigValue::Bool(false)),
        ("overflow".to_owned(), ConfigValue::Bool(false)),
        ("over".to_owned(), ConfigValue::String(key.to_owned())),
        (
            "events".to_owned(),
            ConfigValue::Array(vec![event("down"), event("up")]),
        ),
    ]))
}

fn assert_overlay_is_last_group_order(calls: &[ObservedHostCall], overlay: &str) {
    let group_ops: Vec<_> = calls
        .iter()
        .filter(|call| {
            matches!(
                call.name.as_str(),
                "render.create_group" | "render.raise_groups"
            )
        })
        .collect();
    let Some(last) = group_ops.last() else {
        panic!("the callback changes group order: {calls:?}");
    };
    assert_eq!(last.name, "render.raise_groups", "{calls:?}");
    let [Some(ConfigValue::Array(groups))] = last.arguments.as_slice() else {
        panic!("the final group order is recorded: {last:?}");
    };
    assert_eq!(
        groups.last(),
        Some(&ConfigValue::String(overlay.to_owned())),
        "{calls:?}"
    );
}
