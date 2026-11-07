//! The UI script (`scripts/ui/main.lua`) and its widget library (`scripts/ui/lib/`), run for real over the shipped pixel font and a
//! scene that only records what it is asked to do: the text helpers, the date readout, the hover tooltip and the idle rule.

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ck_lua::{CallOutcome, ConfigValue, Extension, HostFunction, Modules, VmOwner, VmSettings};
use ck_window::{Notifier, Requests};

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

/// The scene calls the UI script makes, recorded: textures load to a token naming the file, quads get tokens `q1`, `q2`, ....
fn scene(log: &Log) -> Vec<Extension> {
    let quads = Arc::new(AtomicUsize::new(0));
    let create: HostFunction = {
        let (log, quads) = (Arc::clone(log), quads);
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
    vec![
        Extension::new("render")
            .function("create_quad", create)
            .function("set_hit", hit_recording(log))
            .function("set_position", recording(log, "set_position"))
            .function("set_rect", recording(log, "set_rect"))
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
    registered: Arc<AtomicBool>,
    hover_registered: Arc<AtomicBool>,
    frame: FrameRegistration,
    controls: std::sync::mpsc::Receiver<Message>,
    sequence: u32,
    _dir: Dir,
}

impl Readout {
    /// The UI script (`scripts/ui/main.lua`, as shipped) followed by `tail`.
    pub(crate) fn new(tail: &str, scale: Option<i64>) -> Self {
        let shipped =
            fs::read_to_string(ui_scripts().join("main.lua")).expect("the shipped UI script");
        Self::script(&format!("{shipped}\n{tail}\n"), scale)
    }

    /// `script` as the UI script.
    pub(crate) fn script(script: &str, scale: Option<i64>) -> Self {
        let dir = Dir::new();
        let entry = dir.0.join("main.lua");
        fs::write(&entry, script).expect("the script");
        let log = Log::default();
        let notifier = Notifier::new();
        let requests = Requests::new();
        let mut window = window_api::extension(requests.writer(Arc::clone(&notifier)), None, None);
        window = window.function("record", recording(&log, "record"));
        let registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let (control_sender, controls) = std::sync::mpsc::channel();
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let shipped_font = Arc::new(font::load(&assets).expect("the shipped font"));
        let config = ConfigValue::Table(match scale {
            Some(scale) => BTreeMap::from([(
                "ui".to_owned(),
                ConfigValue::Table(BTreeMap::from([(
                    "points_per_pixel".to_owned(),
                    ConfigValue::Integer(scale),
                )])),
            )]),
            None => BTreeMap::new(),
        });
        let frame = FrameRegistration::default();
        let mut settings = VmSettings::new("ui", entry, config);
        settings.modules = Modules::Rooted(ui_scripts());
        settings.extensions = vec![window];
        settings.extensions.extend(scene(&log));
        settings.extensions.push(font::extension(shipped_font));
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
        settings.call_limit = 4;
        let mut owner = VmOwner::spawn(settings).expect("the UI VM spawns");
        owner.wait_ready(Duration::from_secs(5)).expect("ready");
        owner.start(Duration::from_secs(5)).expect("started");
        Self {
            owner,
            log,
            registered,
            hover_registered,
            frame,
            controls,
            sequence: 0,
            _dir: dir,
        }
    }

    /// Whether the script has a `frame_update` callback registered.
    pub(crate) fn frame_registered(&self) -> bool {
        self.frame.state().1
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
    }
}

#[test]
fn setting_the_next_hour_replaces_one_quad_and_moves_none() {
    let readout = Readout::new(
        "local l = text.line(10, 20)\ntext.set(l, '14:00')\nwindow.record('mark')\ntext.set(l, '15:00')\nwindow.record('mark')",
        None,
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
        None,
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
        None,
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
        None,
    );
    assert_eq!(
        readout.after_mark(1),
        ["set_position q1 6 8", "set_position q2 18 8"]
    );
    assert!(readout.after_mark(2).is_empty());
    assert_eq!(readout.after_mark(3), ["destroy q1", "destroy q2"]);
}

#[test]
fn the_scale_comes_from_the_ui_table_of_the_configuration() {
    let readout = Readout::new("local l = text.line(0, 0)\ntext.set(l, 'a')", Some(3));
    assert!(
        readout
            .entries()
            .contains(&"create q1 ui/font/glyphs/U+0061.png 0 0 15 33".to_owned()),
        "{:?}",
        readout.entries()
    );
}

#[test]
fn the_readout_format() {
    let readout = Readout::new(
        "window.record(readout_text({year = 768, month = 10, day = 9, hour = 14, speed = 3, paused = false}))\n\
         window.record(readout_text({year = 768, month = 10, day = 9, hour = 14, speed = 3, paused = 'error'}))\n\
         window.record(readout_text({year = 1, month = 1, day = 1, hour = 5, speed = 1, paused = 'load'}))\n\
         window.record(readout_text({year = 9999, month = 12, day = 31, hour = 23, speed = 5, paused = false}))",
        None,
    );
    let records: Vec<String> = readout
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("record "))
        .collect();
    assert_eq!(
        records,
        [
            "record 9 Oct 768 · 14:00 · speed 3",
            "record 9 Oct 768 · 14:00 · paused",
            "record 1 Jan 1 · 05:00 · paused",
            "record 31 Dec 9999 · 23:00 · speed 5",
        ]
    );
}

#[test]
fn the_time_bar_creates_its_panel_first_then_touches_only_what_changed() {
    let mut readout = Readout::new("", None);
    assert!(
        readout.registered.load(Ordering::SeqCst),
        "the script registered game.on_time"
    );
    readout.deliver(&snapshot(14, 1, Some(PauseReason::Load)));
    let first = readout.entries();
    // The panel is at the snapped margin, with the fixed buttons and the paused readout in one row.
    assert_eq!(first[0], "load ui/font/panel.png");
    assert!(
        first[1].starts_with("create q1 ui/font/panel.png 8 8 "),
        "{first:?}"
    );
    // The scene contains the panel, three background quads per each of eight buttons, their nine glyphs, and the readout glyphs.
    let text = "9 Oct 768 · 14:00 · paused";
    let glyphs = text.chars().filter(|c| *c != ' ').count();
    assert_eq!(
        first.iter().filter(|e| e.starts_with("create")).count(),
        glyphs + 1 + 8 * 3 + 9,
        "{first:?}"
    );
    assert_eq!(first[1], "create q1 ui/font/panel.png 8 8 460 38");

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

    // Running at speed 3 is a longer text: the panel follows the text's width.
    let before = readout.entries().len();
    readout.deliver(&snapshot(15, 3, None));
    let third = readout.entries()[before..].to_vec();
    assert!(
        third.iter().any(|e| e.starts_with("set_rect q1 8 8 ")),
        "{third:?}"
    );
}

#[test]
fn hover_tooltip_flips_moves_and_hides_while_reusing_its_quads() {
    let mut readout = Readout::new("", None);
    assert!(readout.hover_registered.load(Ordering::SeqCst));

    readout.deliver_hover(Some(("town", "Alpha", 10, 20, 40)));
    let first = readout.entries();
    assert!(first[0].starts_with("load ui/font/panel.png"), "{first:?}");
    assert!(
        first[1].starts_with("create q1 ui/font/panel.png 24 28 "),
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .any(|line| line.contains("ui/font/glyphs/U+0041.png")),
        "{first:?}"
    );
    let created = first
        .iter()
        .filter(|line| line.starts_with("create"))
        .count();

    let before_move = first.len();
    readout.deliver_hover(Some(("town", "Alpha", 700, 760, 80)));
    let moved = readout.entries()[before_move..].to_vec();
    // Alpha is 28 font pixels wide; the 2-point scale plus 2-pixel panel margin makes a 60-point
    // panel. At left=700, its flipped x is exactly 700 - 4 - 60 = 636. The panel's y, 80 - 13 = 67, snaps to the 2-point grid: 68.
    assert!(
        moved.iter().any(|line| line == "set_rect q1 636 68 60 26"),
        "{moved:?}"
    );
    assert!(
        moved.iter().any(|line| line == "set_position q2 638 70"),
        "{moved:?}"
    );
    assert_eq!(
        moved
            .iter()
            .filter(|line| line.starts_with("create"))
            .count(),
        0,
        "{moved:?}"
    );

    let before_hide = readout.entries().len();
    readout.deliver_hover(None);
    let hidden = readout.entries()[before_hide..].to_vec();
    assert!(
        hidden
            .iter()
            .any(|line| line.starts_with("set_rect q1 -1000000 -1000000 ")),
        "{hidden:?}"
    );
    assert!(
        hidden
            .iter()
            .any(|line| line.contains("set_position q2 -1000000 -1000000")),
        "{hidden:?}"
    );
    assert_eq!(
        hidden
            .iter()
            .filter(|line| line.starts_with("destroy"))
            .count(),
        0,
        "{hidden:?}"
    );
    assert_eq!(
        readout
            .entries()
            .iter()
            .filter(|line| line.starts_with("create"))
            .count(),
        created
    );
}
