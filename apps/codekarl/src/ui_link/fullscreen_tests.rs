//! The fullscreen key (CK-326): `scripts/ui/lib/fullscreen_key.lua` in a real UI VM over the real input path and the production settings
//! runtime, with no window. The runtime's mode request is read from the request slot the window loop would read; nothing here switches a
//! real window to fullscreen (rule C4). Nothing sleeps: every update is awaited.

use ck_window::{CloseSource, PointerEvent, WindowMode};

use super::group_tests::Rig;
use super::listbar_tests::{notes, point, script};
use super::menu_tests::{MENU, close_request, press_mapper_key, release_mapper_key};
use super::widget_witness_tests::points;
use super::*;

/// What `main.lua` does with an update: the fullscreen key first, then a note of the mode the settings report.
const SCRIPT: &str = r#"
fullscreen_key = require("lib.fullscreen_key")
typing = require("lib.typing")
function on_frame(update)
  local asked = fullscreen_key.update(update)
  note("asked " .. tostring(asked) .. " shown " .. settings.graphics().mode)
end
"#;

struct Fixture {
    _directory: crate::testing::Dir,
    rig: Rig,
    settings_path: std::path::PathBuf,
    config_path: std::path::PathBuf,
}

/// A rig over a live settings runtime (a window-mode request slot, as in a run) for a config with a scenario and `window_text` added.
fn fixture(window_text: &str, steps: &[&str]) -> Fixture {
    let directory = crate::testing::Dir::new();
    let config_path = directory.0.join("config.toml");
    std::fs::write(
        &config_path,
        format!("[game]\nscenario = '.'\n{window_text}"),
    )
    .expect("private test config");
    let config = crate::config::load(&config_path).expect("load private config");
    let settings_path = config.settings_file.clone().expect("settings path");
    let requests = ck_window::Requests::new();
    let reader = requests.reader();
    let writer = requests.writer(ck_window::Notifier::new());
    // The window the script would have opened (the rig attaches its scene by hand): resizable, as the mode request requires.
    writer
        .open(ck_window::WindowDesc {
            title: "fullscreen key".to_owned(),
            width: 1280,
            height: 800,
            resizable: true,
            ..ck_window::WindowDesc::default()
        })
        .expect("the window is requested");
    let _ = reader.take();
    let resize = crate::window_api::ResizeSession::new(
        writer.clone(),
        Arc::new(std::sync::OnceLock::new()),
        None,
    );
    let runtime = crate::settings::Runtime::new_live(
        &config,
        None,
        writer,
        ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest),
        None,
        resize,
        ck_render_2d::WakeRelay::default(),
        crate::place_names::PlaceNameSetting::default(),
    )
    .expect("live settings runtime");
    let script = script(&format!("{MENU}\n{SCRIPT}"), steps);
    let mut rig = Rig::new_fixture_body_with_requests(&script, runtime.extension(), reader);
    rig.mapper.replace_bindings(config.bindings.clone());
    rig.deliver_fixture_frame_at(800, 600, 2);
    Fixture {
        _directory: directory,
        rig,
        settings_path,
        config_path,
    }
}

impl Fixture {
    /// Presses and releases F11 through the real mapper, and returns the mode request the press queued, if any.
    fn tap(&mut self) -> Option<WindowMode> {
        press_mapper_key(&mut self.rig, ck_input::Key::F11);
        let mode = self.rig.apply_settings_requests().mode;
        release_mapper_key(&mut self.rig, ck_input::Key::F11);
        mode.map(|layout| layout.mode)
    }

    /// The mode in the settings file, if it holds one.
    fn saved(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.settings_path).ok()?;
        let value: toml::Value = toml::from_str(&text).expect("settings TOML parses");
        value
            .get("window")?
            .get("mode")?
            .as_str()
            .map(str::to_owned)
    }

    /// The last note the frame hook wrote: what the press asked for and the mode the settings showed.
    fn last_note(&self) -> String {
        notes(&self.rig)
            .into_iter()
            .rfind(|text| text.starts_with("asked "))
            .expect("a frame ran")
    }

    /// Runs the next queued script step, then a frame.
    fn step(&mut self) {
        self.rig.opened();
        self.rig.deliver_fixture_frame();
    }
}

#[test]
fn f11_switches_windowed_to_fullscreen_and_back_and_keeps_the_choice() {
    let mut f = fixture("", &[]);
    assert_eq!(f.tap(), Some(WindowMode::Fullscreen));
    assert!(
        f.last_note().ends_with("shown fullscreen"),
        "{}",
        f.last_note()
    );
    assert_eq!(f.saved().as_deref(), Some("fullscreen"));
    // A restart reads the saved mode.
    let restarted = crate::config::load(&f.config_path).expect("restart");
    assert_eq!(restarted.graphics.mode, WindowMode::Fullscreen);

    assert_eq!(f.tap(), Some(WindowMode::Windowed));
    assert!(
        f.last_note().ends_with("shown windowed"),
        "{}",
        f.last_note()
    );
    let restarted = crate::config::load(&f.config_path).expect("restart");
    assert_eq!(restarted.graphics.mode, WindowMode::Windowed);
}

#[test]
fn f11_in_windowed_fullscreen_asks_for_windowed() {
    let mut f = fixture("[window]\nmode = 'windowed_fullscreen'\n", &[]);
    assert_eq!(f.tap(), Some(WindowMode::Windowed));
    assert!(
        f.last_note().ends_with("shown windowed"),
        "{}",
        f.last_note()
    );
    assert_eq!(f.saved().as_deref(), Some("windowed"));
}

#[test]
fn f11_is_the_default_binding_of_the_fullscreen_action() {
    let f = fixture("", &[]);
    let bindings = f.rig.mapper.bindings();
    let action = bindings
        .action("window_fullscreen")
        .expect("the action is bound with a scenario");
    let keys: Vec<_> = action.bindings().iter().map(|b| b.key().name()).collect();
    assert_eq!(keys, ["F11"]);
}

#[test]
fn the_fullscreen_key_can_be_rebound_and_the_defaults_restore_f11() {
    let mut f = fixture(
        "",
        &[
            "settings.set_bindings{window_fullscreen = {'F10'}}",
            "settings.reset('bindings')",
        ],
    );
    f.step();
    f.rig.apply_settings_requests();
    assert_eq!(
        f.tap(),
        None,
        "F11 no longer asks once the action is rebound"
    );
    f.rig.mapper.key(ck_input::Key::F10, true, false);
    let snapshot = f.rig.mapper.snapshot();
    let sequence = snapshot.sequence;
    f.rig
        .link
        .input(&input_tick_tests::tick(snapshot))
        .expect("tick");
    f.rig.mapper.acknowledge(sequence);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        Some(WindowMode::Fullscreen),
        "the new key asks"
    );
    f.rig.mapper.key(ck_input::Key::F10, false, false);
    f.step();
    f.rig.apply_settings_requests();
    assert_eq!(
        f.tap(),
        Some(WindowMode::Windowed),
        "the defaults restored F11"
    );
}

#[test]
fn f11_does_nothing_while_the_menu_is_open_typing_or_a_key_capture_waits() {
    let mut f = fixture(
        "",
        &[
            "menu.open(VIEW)",
            "menu.close()",
            "editor = {on_typed = function() end}; typing.start(editor)",
            "typing.stop(editor)",
            "typing.capture('t', settings.capture_key)\nnote('blocked ' .. tostring(fullscreen_key.blocked()))",
            "typing.cancel_capture(settings.cancel_capture)",
        ],
    );
    // The press works with none of these on (so each case below proves its block), and ends in windowed again.
    assert_eq!(f.tap(), Some(WindowMode::Fullscreen));
    assert_eq!(f.tap(), Some(WindowMode::Windowed));

    f.step();
    assert_eq!(f.tap(), None, "the menu is open");
    f.step();

    f.step();
    assert_eq!(f.tap(), None, "a widget is editing");
    f.step();

    // An armed capture takes the key itself, so it never reaches the action; the guard also holds on its own for an update that
    // carries a completed capture and the press.
    f.step();
    f.rig.apply_settings_requests();
    assert!(notes(&f.rig).contains(&"blocked true".to_owned()));
    f.step();
    f.rig.apply_settings_requests();

    // Everything off again: the key works.
    assert_eq!(f.tap(), Some(WindowMode::Fullscreen));
}

/// The frame hook of `main.lua`, in its order: typing, then the pointer, then the settings window, then the fullscreen key.
const PRODUCTION_ORDER: &str = r#"
frame.on_update(function(update)
  typing.dispatch(update)
  pointer.dispatch(update.pointer)
  require("lib.settings_window").update(update)
  fullscreen_key.update(update)
end)
"#;

#[test]
fn a_press_made_with_the_menu_open_does_not_act_when_the_same_update_closes_the_menu() {
    let mut f = fixture("", &["menu.open(VIEW); mark_buttons()"]);
    f.step();
    let (x, y) = point(&f.rig, "return");
    let at = points(x, y);
    f.rig.mapper.key(ck_input::Key::F11, true, false);
    // F11 and a click on the menu's Return button reach one update: the pointer closes the menu before the key is looked at.
    f.rig.feed(&[PointerEvent::Down(at), PointerEvent::Up(at)]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        None,
        "F11 was pressed with the menu open"
    );
    release_mapper_key(&mut f.rig, ck_input::Key::F11);
    assert_eq!(f.tap(), Some(WindowMode::Fullscreen), "a fresh press acts");
}

#[test]
fn a_press_that_applies_a_field_does_not_also_act_when_fullscreen_is_rebound_to_enter() {
    let mut f = fixture(
        "[input.bindings]\nwindow_fullscreen = ['Enter']\n",
        &[&format!(
            "{PRODUCTION_ORDER}\n{}",
            r#"
editor = {on_typed = function(self, entry)
  if entry.kind == "enter" then typing.stop(self) end
end}
typing.start(editor)
"#
        )],
    );
    f.step();
    f.rig.apply_settings_requests();
    // The rig applies the first text-input generation by hand, as `typing_tests` does.
    f.rig.mapper.reset_typed(1);
    f.rig.mapper.key(ck_input::Key::Enter, true, false);
    f.rig.mapper.typed(ck_input::TypedEntry::Enter);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        None,
        "Enter belonged to the editing, so applying the field is not also a fullscreen press"
    );
    f.rig.mapper.key(ck_input::Key::Enter, false, false);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    f.rig.apply_settings_requests();
    f.rig.mapper.key(ck_input::Key::Enter, true, false);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        Some(WindowMode::Fullscreen),
        "with the editing over, a fresh Enter acts"
    );
}

#[test]
fn a_key_capture_claims_f11_and_a_fresh_press_after_it_acts() {
    let mut f = fixture(
        "",
        &[&format!(
            "{PRODUCTION_ORDER}\ntyping.capture('review', settings.capture_key)"
        )],
    );
    f.step();
    f.rig.apply_settings_requests();
    assert_eq!(f.tap(), None, "the captured F11 never arrives as an action");
    assert_eq!(
        f.tap(),
        Some(WindowMode::Fullscreen),
        "after it a press acts"
    );
}

#[test]
fn a_press_made_after_escape_opened_the_menu_between_updates_does_not_act_when_its_update_closes_it()
 {
    let mut f = fixture("", &["mark_buttons()"]);
    // Escape opens the menu in the close-request callback; no update runs before the next one.
    close_request(&mut f.rig, CloseSource::Key);
    f.rig.opened();
    let (x, y) = point(&f.rig, "return");
    let at = points(x, y);
    f.rig.mapper.key(ck_input::Key::F11, true, false);
    f.rig.feed(&[PointerEvent::Down(at), PointerEvent::Up(at)]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        None,
        "F11 was pressed with the menu open, though no update saw it open"
    );
    release_mapper_key(&mut f.rig, ck_input::Key::F11);
    assert_eq!(f.tap(), Some(WindowMode::Fullscreen), "a fresh press acts");
}

#[test]
fn a_press_made_after_an_editing_began_between_updates_does_not_act_when_its_update_applies_it() {
    let mut f = fixture(
        "[input.bindings]\nwindow_fullscreen = ['Enter']\n",
        &[
            PRODUCTION_ORDER,
            r#"
editor = {on_typed = function(self, entry)
  if entry.kind == "enter" then typing.stop(self) end
end}
typing.start(editor)
"#,
        ],
    );
    f.step();
    f.rig.apply_settings_requests();
    // The editing starts in a callback, with no update after it.
    f.rig.opened();
    f.rig.mapper.reset_typed(1);
    f.rig.mapper.key(ck_input::Key::Enter, true, false);
    f.rig.mapper.typed(ck_input::TypedEntry::Enter);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        None,
        "Enter belonged to an editing no update had seen"
    );
    f.rig.mapper.key(ck_input::Key::Enter, false, false);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    f.rig.apply_settings_requests();
    f.rig.mapper.key(ck_input::Key::Enter, true, false);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        Some(WindowMode::Fullscreen),
        "with the editing over, a fresh Enter acts"
    );
}

#[test]
fn an_editing_that_started_before_any_update_ran_owns_a_rebound_enter() {
    let mut f = fixture(
        "[input.bindings]\nwindow_fullscreen = ['Enter']\n",
        &[&format!(
            "{PRODUCTION_ORDER}\n{}",
            r#"
editor = {on_typed = function(self, entry)
  if entry.kind == "enter" then typing.stop(self) end
end}
typing.start(editor)
"#
        )],
    );
    // The callback starts the editing and no frame samples it before the Enter arrives.
    f.rig.opened();
    f.rig.apply_settings_requests();
    f.rig.mapper.reset_typed(1);
    f.rig.mapper.key(ck_input::Key::Enter, true, false);
    f.rig.mapper.typed(ck_input::TypedEntry::Enter);
    f.rig.feed(&[]);
    f.rig.deliver_fixture_frame();
    assert_eq!(
        f.rig
            .apply_settings_requests()
            .mode
            .map(|layout| layout.mode),
        None,
        "the editor started and owns Enter even before an update has seen its state"
    );
}
