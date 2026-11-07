//! The `game` extension of the UI VM: how the simulation reaches the UI.
//!
//! `game.on_time(fn | nil)` registers the callback `game_time` (and exists only when the configuration has `[game] scenario`).
//! The UI link posts it with the latest [`SimSnapshot`] as a table, `{year, month, day, hour, speed, paused}`, where `paused`
//! is `false` or the reason as the string `"player"`, `"error"` or `"load"`: see `ui_link` for when.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ck_lua::{ConfigValue, Extension, HostFunction};

use crate::game::controls::{self, SimControls};
use crate::game::slot::{PauseReason, SimSnapshot};

/// The callback `game.on_time` registers.
pub(crate) const GAME_TIME: &str = "game_time";
/// The callback `game.on_hover` registers.
pub(crate) const GAME_HOVER: &str = "game_hover";

/// The `game` extension. `registered` follows whether a callback is set; `wake` runs after each change so the loop's next pass
/// delivers the current state to a callback that has just been registered.
pub(crate) fn extension(
    time_registered: Arc<AtomicBool>,
    hover_registered: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
    controls: SimControls,
) -> Extension {
    let time_wake = Arc::clone(&wake);
    Extension::new("game")
        .function("control", control_function(controls))
        .callback_observed(
            "on_time",
            GAME_TIME,
            Arc::new(move |is_registered| {
                time_registered.store(is_registered, Ordering::SeqCst);
                time_wake();
            }),
        )
        .callback_observed(
            "on_hover",
            GAME_HOVER,
            Arc::new(move |is_registered| {
                hover_registered.store(is_registered, Ordering::SeqCst);
                wake();
            }),
        )
}

/// `game.control(name)` sends the named time or save control over the run's simulation channel.
fn control_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::String(name))] = arguments else {
            return Err("game.control: expected one string argument (name)".to_owned());
        };
        let Some(control) = controls::control_of(name) else {
            return Err(format!("game.control: unknown control name `{name}`"));
        };
        controls.send_control(control);
        Ok(Vec::new())
    })
}

/// The callback argument for a hovered marker; `None` means Lua receives no arguments (`nil`).
pub(crate) fn hover_arguments(hover: Option<&crate::map_app::Hover>) -> Vec<ConfigValue> {
    let Some(hover) = hover else {
        return Vec::new();
    };
    vec![ConfigValue::Table(BTreeMap::from([
        ("id".to_owned(), ConfigValue::String(hover.id.clone())),
        ("name".to_owned(), ConfigValue::String(hover.name.clone())),
        ("left".to_owned(), ConfigValue::Integer(hover.left)),
        ("right".to_owned(), ConfigValue::Integer(hover.right)),
        ("y".to_owned(), ConfigValue::Integer(hover.y)),
    ]))]
}

/// The table a `game_time` callback gets for `snapshot`.
pub(crate) fn time_table(snapshot: &SimSnapshot) -> ConfigValue {
    let paused = match snapshot.paused {
        None => ConfigValue::Bool(false),
        Some(PauseReason::Player) => ConfigValue::String("player".to_owned()),
        Some(PauseReason::Error) => ConfigValue::String("error".to_owned()),
        Some(PauseReason::Load) => ConfigValue::String("load".to_owned()),
    };
    ConfigValue::Table(BTreeMap::from([
        (
            "year".to_owned(),
            ConfigValue::Integer(i64::from(snapshot.date.year)),
        ),
        (
            "month".to_owned(),
            ConfigValue::Integer(i64::from(snapshot.date.month)),
        ),
        (
            "day".to_owned(),
            ConfigValue::Integer(i64::from(snapshot.date.day)),
        ),
        (
            "hour".to_owned(),
            ConfigValue::Integer(i64::from(snapshot.date.hour)),
        ),
        (
            "speed".to_owned(),
            ConfigValue::Integer(i64::from(snapshot.speed.level())),
        ),
        ("paused".to_owned(), paused),
    ]))
}
