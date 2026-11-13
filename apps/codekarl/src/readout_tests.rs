//! The UI script (`scripts/ui/main.lua`) and its widget library (`scripts/ui/lib/`), run for real over the shipped pixel font and a
//! scene that only records what it is asked to do: the text helpers, the date readout, the hover tooltip and the idle rule.

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

use ck_lua::{CallOutcome, ConfigValue, Extension, HostFunction, Modules, VmOwner, VmSettings};
use ck_window::{Notifier, Requests};

use crate::config::TooltipsConfig;
use crate::frame_api::{self, FrameBackend, FrameRegistration};
use crate::game::core::Message;
use crate::game::slot::{PauseReason, SimSnapshot};
use crate::testing::Dir;
use crate::window_api::FRAME_UPDATE;
use crate::{font, game_api, window_api};

type Log = Arc<Mutex<Vec<String>>>;

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

/// `render.set_hit`, recorded as `set_hit <key> <x> <y> <width> <height>` per rectangle, or `set_hit <key> nil`.
fn hit_recording(log: &Log) -> HostFunction {
    let log = Arc::clone(log);
    Arc::new(move |arguments| {
        let Some(Some(ConfigValue::String(key))) = arguments.first() else {
            return Err("render.set_hit: a key".to_owned());
        };
        if !ck_render_2d::valid_key(key) {
            return Err("render.set_hit: key must match [a-z0-9_]{1,48}".to_owned());
        }
        match arguments.get(1).and_then(Option::as_ref) {
            Some(ConfigValue::Array(rects)) => {
                for rect in rects {
                    push(
                        &log,
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
            _ => push(&log, format!("set_hit {key} nil")),
        }
        Ok(Vec::new())
    })
}

#[test]
fn the_readout_hit_double_matches_the_render_key_contract() {
    const ERROR: &str = "render.set_hit: key must match [a-z0-9_]{1,48}";
    let log = Arc::new(Mutex::new(Vec::new()));
    let set_hit = hit_recording(&log);
    let invalid = ["a:b".to_owned(), String::new(), "x".repeat(49)];
    for key in invalid {
        assert_eq!(
            set_hit(&[Some(ConfigValue::String(key)), None]).expect_err("invalid keys are refused"),
            ERROR
        );
    }
    let accepted = "x".repeat(48);
    set_hit(&[Some(ConfigValue::String(accepted.clone())), None])
        .expect("a key of the maximum length is accepted");
}

/// The scene calls the UI script makes, recorded: textures load to a token naming the file, quads get tokens `q1`, `q2`, ....
/// The recording scene. Positions and sizes go to `log`; the tints (a quad's at creation, then every `render.set_tint`) go to `tints`,
/// so the geometry literals of the tests are not cluttered by colours.
fn scene(log: &Log, tints: &Log) -> Vec<Extension> {
    let quads = Arc::new(AtomicUsize::new(0));
    let groups = Arc::new(AtomicUsize::new(0));
    let create: HostFunction = {
        let (log, quads, tints) = (Arc::clone(log), quads, Arc::clone(tints));
        Arc::new(move |arguments| {
            let Some(Some(table)) = arguments.first() else {
                return Err("render.create_quad: a table".to_owned());
            };
            let number = quads.fetch_add(1, Ordering::SeqCst) + 1;
            let texture = field(table, "texture");
            push(
                &log,
                format!(
                    "create q{number} {} {} {} {} {}",
                    texture.trim_start_matches("t:"),
                    field(table, "x"),
                    field(table, "y"),
                    field(table, "width"),
                    field(table, "height")
                ),
            );
            let tint = field(table, "tint");
            if !tint.is_empty() {
                push(&tints, format!("create q{number} {tint}"));
            }
            Ok(vec![ConfigValue::String(format!("q{number}"))])
        })
    };
    let load: HostFunction = {
        let log = Arc::clone(log);
        Arc::new(move |arguments| {
            let Some(Some(ConfigValue::String(name))) = arguments.first() else {
                return Err("texture.load: a name".to_owned());
            };
            push(&log, format!("load {name}"));
            Ok(vec![ConfigValue::String(format!("t:{name}"))])
        })
    };
    let create_group: HostFunction = {
        let (log, groups) = (Arc::clone(log), groups);
        Arc::new(move |_| {
            let number = groups.fetch_add(1, Ordering::SeqCst) + 1;
            push(&log, format!("create_group g{number}"));
            Ok(vec![ConfigValue::String(format!("g{number}"))])
        })
    };
    let raise_groups: HostFunction = Arc::new(|_| Ok(Vec::new()));
    vec![
        Extension::new("render")
            .function("create_group", create_group)
            .function("raise_groups", raise_groups)
            .function("create_quad", create)
            .function("set_hit", hit_recording(log))
            .function("set_position", recording(log, "set_position"))
            .function("set_rect", recording(log, "set_rect"))
            .function("set_tint", recording(tints, "set_tint"))
            .function("set_group_clip", recording(log, "set_group_clip"))
            .function("set_group_offset", recording(log, "set_group_offset"))
            .function("set_group_visible", recording(log, "set_group_visible"))
            .function("destroy_group", recording(log, "destroy_group"))
            .function("destroy_quad", recording(log, "destroy")),
        Extension::new("texture").function("load", load),
    ]
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
        settings.extensions.extend(scene(&log, &tints));
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
    for _ in 0..40 {
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
