//! The player's untracked UI settings file and the calls the UI VM uses to read and change it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use ck_log::Level;
use ck_lua::{ConfigValue, Extension};
use ck_window::Bindings;

use crate::config::{Config, MapSettings, SettingsOverrides, TooltipsConfig};
use crate::lifecycle::emit;
use crate::map_app::{self, SharedController};

#[derive(Clone, Copy)]
struct Values {
    tooltips: TooltipsConfig,
    map: MapSettings,
}

/// Thread-safe live settings state shared by UI host calls and the map controller.
pub(crate) struct Runtime {
    path: PathBuf,
    base: Values,
    values: Mutex<Values>,
    overrides: Mutex<SettingsOverrides>,
    bindings: Bindings,
    map: Option<SharedController>,
}

impl Runtime {
    pub(crate) fn new(config: &Config, map: Option<SharedController>) -> Option<Arc<Self>> {
        Some(Arc::new(Self {
            path: config.settings_file.clone()?,
            base: Values {
                tooltips: config.settings_base_tooltips,
                map: config.settings_base_map,
            },
            values: Mutex::new(Values {
                tooltips: config.tooltips,
                map: config.map_settings,
            }),
            overrides: Mutex::new(config.settings_overrides.clone()),
            bindings: config.bindings.clone(),
            map,
        }))
    }

    pub(crate) fn extension(self: &Arc<Self>) -> Extension {
        let tooltips = Arc::clone(self);
        let map = Arc::clone(self);
        let set = Arc::clone(self);
        let reset = Arc::clone(self);
        let bindings = Arc::clone(self);
        Extension::new("settings")
            .function(
                "tooltips",
                Arc::new(move |args| {
                    expect_no_arguments("settings.tooltips", args)?;
                    let values = *tooltips
                        .values
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    Ok(vec![tooltip_value(values.tooltips)])
                }),
            )
            .function(
                "map",
                Arc::new(move |args| {
                    expect_no_arguments("settings.map", args)?;
                    let values = *map.values.lock().unwrap_or_else(PoisonError::into_inner);
                    Ok(vec![map_value(values.map)])
                }),
            )
            .function("set", Arc::new(move |args| set.set(args)))
            .function("reset", Arc::new(move |args| reset.reset(args)))
            .function(
                "bindings",
                Arc::new(move |args| {
                    expect_no_arguments("settings.bindings", args)?;
                    Ok(vec![bindings_value(&bindings.bindings)])
                }),
            )
    }

    fn set(&self, args: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        if args.len() != 2 {
            return Err("settings.set: expected a setting name and value".to_owned());
        }
        let Some(ConfigValue::String(name)) = args[0].as_ref() else {
            return Err("settings.set: setting name must be a string".to_owned());
        };
        let value = args[1]
            .as_ref()
            .ok_or_else(|| format!("settings.set: `{name}` requires a value"))?;
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        let mut overrides = self
            .overrides
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match name.as_str() {
            "ui.tooltips.delay" => {
                let tenths = tooltip_number(name, value, 0, 20)?;
                values.tooltips.delay = f64::from(tenths) / 10.0;
                overrides.tooltip_delay = Some(values.tooltips.delay);
            }
            "ui.tooltips.lock_timer" => {
                let tenths = tooltip_number(name, value, 3, 30)?;
                values.tooltips.lock_timer = f64::from(tenths) / 10.0;
                overrides.tooltip_lock_timer = Some(values.tooltips.lock_timer);
            }
            "ui.tooltips.lock" => {
                let ConfigValue::String(value) = value else {
                    return Err(format!("settings.set: `{name}` must be a string"));
                };
                values.tooltips.lock = match value.as_str() {
                    "timer" => "timer",
                    "middle_mouse" => "middle_mouse",
                    "disabled" => "disabled",
                    _ => {
                        return Err(format!(
                            "settings.set: `{name}` value {value:?} is not accepted: use timer, middle_mouse or disabled"
                        ));
                    }
                };
                overrides.tooltip_lock = Some(values.tooltips.lock);
            }
            "map.pan_pixels_per_frame" => {
                values.map.pan_pixels_per_frame = map_integer(name, value, 1, 32)?;
                overrides.map_pan_pixels_per_frame = Some(values.map.pan_pixels_per_frame);
            }
            "map.zoom_steps_per_second" => {
                values.map.zoom_steps_per_second = map_integer(name, value, 1, 60)?;
                overrides.map_zoom_steps_per_second = Some(values.map.zoom_steps_per_second);
            }
            "map.scroll_points_per_step" => {
                values.map.scroll_points_per_step = map_integer(name, value, 1, 256)?;
                overrides.map_scroll_points_per_step = Some(values.map.scroll_points_per_step);
            }
            _ => return Err(format!("settings.set: unknown setting `{name}`")),
        }
        self.apply_map(values.map);
        self.persist(&overrides);
        Ok(Vec::new())
    }

    fn reset(&self, args: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        if args.len() != 1 {
            return Err("settings.reset: expected `tooltips` or `map`".to_owned());
        }
        let Some(ConfigValue::String(tab)) = args[0].as_ref() else {
            return Err("settings.reset: tab must be a string".to_owned());
        };
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        let mut overrides = self
            .overrides
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match tab.as_str() {
            "tooltips" => {
                values.tooltips = self.base.tooltips;
                overrides.tooltip_delay = None;
                overrides.tooltip_lock = None;
                overrides.tooltip_lock_timer = None;
            }
            "map" => {
                values.map = self.base.map;
                overrides.map_pan_pixels_per_frame = None;
                overrides.map_zoom_steps_per_second = None;
                overrides.map_scroll_points_per_step = None;
                self.apply_map(values.map);
            }
            _ => {
                return Err(format!(
                    "settings.reset: unknown tab `{tab}`; use tooltips or map"
                ));
            }
        }
        self.persist(&overrides);
        Ok(Vec::new())
    }

    fn apply_map(&self, settings: MapSettings) {
        if let Some(map) = &self.map {
            map_app::lock(map).set_controls(
                settings.pan_pixels_per_frame,
                settings.zoom_steps_per_second,
                settings.scroll_points_per_step,
            );
        }
    }

    fn persist(&self, overrides: &SettingsOverrides) {
        if let Err(error) =
            crate::game::saves::write_atomically(&self.path, &settings_text(overrides))
        {
            emit(
                Level::Warn,
                "config",
                format_args!("{}: could not write settings: {error}", self.path.display()),
            );
        }
    }
}

fn expect_no_arguments(name: &str, args: &[Option<ConfigValue>]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("{name}: expected no arguments"))
    }
}

fn tooltip_value(settings: TooltipsConfig) -> ConfigValue {
    ConfigValue::Table(std::collections::BTreeMap::from([
        ("delay".to_owned(), ConfigValue::Float(settings.delay)),
        (
            "lock".to_owned(),
            ConfigValue::String(settings.lock.to_owned()),
        ),
        (
            "lock_timer".to_owned(),
            ConfigValue::Float(settings.lock_timer),
        ),
    ]))
}

fn map_value(settings: MapSettings) -> ConfigValue {
    ConfigValue::Table(std::collections::BTreeMap::from([
        (
            "pan_pixels_per_frame".to_owned(),
            ConfigValue::Integer(i64::from(settings.pan_pixels_per_frame)),
        ),
        (
            "zoom_steps_per_second".to_owned(),
            ConfigValue::Integer(i64::from(settings.zoom_steps_per_second)),
        ),
        (
            "scroll_points_per_step".to_owned(),
            ConfigValue::Integer(i64::from(settings.scroll_points_per_step)),
        ),
    ]))
}

fn bindings_value(bindings: &Bindings) -> ConfigValue {
    ConfigValue::Array(
        bindings
            .actions()
            .iter()
            .map(|action| {
                let entries = action
                    .bindings()
                    .iter()
                    .map(|binding| {
                        let mut parts = Vec::new();
                        if binding.requires_control() {
                            parts.push("Control");
                        }
                        if binding.requires_alt() {
                            parts.push("Alt");
                        }
                        if binding.requires_shift() {
                            parts.push("Shift");
                        }
                        if binding.requires_meta() {
                            parts.push("Meta");
                        }
                        parts.push(binding.key().name());
                        ConfigValue::String(parts.join("+"))
                    })
                    .collect();
                ConfigValue::Table(std::collections::BTreeMap::from([
                    (
                        "name".to_owned(),
                        ConfigValue::String(action.name().to_owned()),
                    ),
                    ("bindings".to_owned(), ConfigValue::Array(entries)),
                ]))
            })
            .collect(),
    )
}

fn tooltip_number(
    name: &str,
    value: &ConfigValue,
    minimum: u32,
    maximum: u32,
) -> Result<u32, String> {
    let number = match value {
        ConfigValue::Integer(number) => u32::try_from(*number)
            .ok()
            .filter(|number| *number <= 3)
            .map(f64::from),
        ConfigValue::Float(number) => Some(*number),
        _ => None,
    };
    number
        .and_then(|number| crate::config::checked_tooltip_tenths(number, minimum, maximum))
        .ok_or_else(|| {
            format!(
                "settings.set: `{name}` must be in {}..={} in 0.1-second steps",
                f64::from(minimum) / 10.0,
                f64::from(maximum) / 10.0
            )
        })
}

fn map_integer(name: &str, value: &ConfigValue, minimum: u32, maximum: u32) -> Result<u32, String> {
    match value {
        ConfigValue::Integer(number) => u32::try_from(*number)
            .ok()
            .filter(|n| (minimum..=maximum).contains(n)),
        _ => None,
    }
    .ok_or_else(|| format!("settings.set: `{name}` must be an integer in {minimum}..={maximum}"))
}

fn settings_text(overrides: &SettingsOverrides) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut text = String::new();
    if overrides.tooltip_delay.is_some()
        || overrides.tooltip_lock.is_some()
        || overrides.tooltip_lock_timer.is_some()
    {
        text.push_str("[ui.tooltips]\n");
        if let Some(value) = overrides.tooltip_delay {
            let _ = writeln!(text, "delay = {value:.1}");
        }
        if let Some(value) = overrides.tooltip_lock {
            let _ = writeln!(text, "lock = \"{value}\"");
        }
        if let Some(value) = overrides.tooltip_lock_timer {
            let _ = writeln!(text, "lock_timer = {value:.1}");
        }
        text.push('\n');
    }
    if overrides.map_pan_pixels_per_frame.is_some()
        || overrides.map_zoom_steps_per_second.is_some()
        || overrides.map_scroll_points_per_step.is_some()
    {
        text.push_str("[map]\n");
        if let Some(value) = overrides.map_pan_pixels_per_frame {
            let _ = writeln!(text, "pan_pixels_per_frame = {value}");
        }
        if let Some(value) = overrides.map_zoom_steps_per_second {
            let _ = writeln!(text, "zoom_steps_per_second = {value}");
        }
        if let Some(value) = overrides.map_scroll_points_per_step {
            let _ = writeln!(text, "scroll_points_per_step = {value}");
        }
    }
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn ui_calls_set_read_reset_and_persist_without_changing_other_tab_values() {
        let dir = crate::testing::Dir::new();
        let config_path = dir.0.join("config.toml");
        fs::write(
            &config_path,
            "[game]\nscenario = 'scenario'\n\n[ui.tooltips]\ndelay = 0.3\n\n[input.bindings]\ntime_pause = ['Control+KeyP']\ngame_quickload = []\n",
        )
        .expect("write config");
        let config = crate::config::load(&config_path).expect("load config and settings");
        let path = config.settings_file.clone().expect("settings path");
        let runtime = Runtime::new(&config, None).expect("runtime settings");
        let script = r#"
assert(settings.tooltips().delay == 0.3)
assert(settings.map().pan_pixels_per_frame == 4)
settings.set("ui.tooltips.delay", 3 * 0.1)
assert(math.abs(settings.tooltips().delay - 0.3) < 1e-9)
settings.set("ui.tooltips.delay", 0.8)
settings.set("map.pan_pixels_per_frame", 9)
assert(settings.tooltips().delay == 0.8)
assert(settings.map().pan_pixels_per_frame == 9)
local ok, message = pcall(function() settings.set("ui.tooltips.delay", 0.25) end)
assert(not ok and string.find(tostring(message), "ui.tooltips.delay", 1, true))
assert(settings.tooltips().delay == 0.8)
local actions = settings.bindings()
assert(#actions == 22)
assert(actions[1].name == "close" and actions[1].bindings[1] == "Escape")
local pause, quickload
for _, action in ipairs(actions) do
  if action.name == "time_pause" then pause = action end
  if action.name == "game_quickload" then quickload = action end
end
assert(pause.bindings[1] == "Control+KeyP")
assert(#quickload.bindings == 0)
settings.reset("tooltips")
assert(settings.tooltips().delay == 0.3)
assert(settings.map().pan_pixels_per_frame == 9)
"#;
        let harness = crate::testing::Harness::new(
            script,
            crate::testing::Extras {
                extensions: vec![runtime.extension()],
                ..Default::default()
            },
        );

        let written = fs::read_to_string(&path).expect("settings were written synchronously");
        let written: toml::Value = toml::from_str(&written).expect("written TOML parses");
        assert_eq!(written["map"]["pan_pixels_per_frame"].as_integer(), Some(9));
        assert!(
            written.get("ui").is_none(),
            "tooltips reset removes only its table"
        );
        assert!(!path.with_extension("toml.tmp").exists());
        drop(harness);

        let second_start = crate::config::load(&config_path).expect("second start reads settings");
        assert!(second_start.settings_warnings.is_empty());
        assert!((second_start.tooltips.delay - 0.3).abs() < 1.0e-9);
        assert_eq!(second_start.map_settings.pan_pixels_per_frame, 9);
    }

    #[test]
    fn rejected_sets_name_the_setting_and_write_nothing() {
        let dir = crate::testing::Dir::new();
        let config_path = dir.0.join("config.toml");
        fs::write(&config_path, "[ui.tooltips]\ndelay = 0.3\n").expect("write config");
        let config = crate::config::load(&config_path).expect("load config and settings");
        let path = config.settings_file.clone().expect("settings path");
        let runtime = Runtime::new(&config, None).expect("runtime settings");
        let script = r#"
for _, item in ipairs({
  {"ui.tooltips.delay", 0.25},
  {"ui.tooltips.lock", "never"},
  {"map.pan_pixels_per_frame", 0},
  {"map.directory", "other"},
  {"not.a.setting", 1},
}) do
  local ok, message = pcall(function() settings.set(item[1], item[2]) end)
  assert(not ok and string.find(tostring(message), item[1], 1, true))
end
assert(settings.tooltips().delay == 0.3)
assert(settings.map().pan_pixels_per_frame == 4)
"#;
        let _harness = crate::testing::Harness::new(
            script,
            crate::testing::Extras {
                extensions: vec![runtime.extension()],
                ..Default::default()
            },
        );
        assert!(!path.exists(), "a refused setting does not create the file");
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
