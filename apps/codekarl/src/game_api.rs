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
/// The callback `game.on_select` registers.
pub(crate) const GAME_SELECT: &str = "game_select";

/// The `game` extension. `registered` follows whether a callback is set; `wake` runs after each change so the loop's next pass
/// delivers the current state to a callback that has just been registered.
#[cfg(test)]
pub(crate) fn extension(
    time_registered: Arc<AtomicBool>,
    hover_registered: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
    controls: SimControls,
) -> Extension {
    extension_with_select(
        time_registered,
        hover_registered,
        Arc::new(AtomicBool::new(false)),
        wake,
        controls,
    )
}

pub(crate) fn extension_with_select(
    time_registered: Arc<AtomicBool>,
    hover_registered: Arc<AtomicBool>,
    select_registered: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
    controls: SimControls,
) -> Extension {
    let time_wake = Arc::clone(&wake);
    let hover_wake = Arc::clone(&wake);
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
                hover_wake();
            }),
        )
        .callback_observed(
            "on_select",
            GAME_SELECT,
            Arc::new(move |is_registered| {
                select_registered.store(is_registered, Ordering::SeqCst);
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

/// The argument for a selected place. Selection callbacks have no nil notification.
pub(crate) fn select_arguments(selection: &crate::map_app::Selection) -> Vec<ConfigValue> {
    vec![ConfigValue::Table(BTreeMap::from([
        (
            "seq".to_owned(),
            ConfigValue::Integer(
                i64::try_from(selection.seq).expect("selection sequence fits a Lua integer"),
            ),
        ),
        ("id".to_owned(), ConfigValue::String(selection.id.clone())),
        (
            "name".to_owned(),
            ConfigValue::String(selection.name.clone()),
        ),
        (
            "kind".to_owned(),
            ConfigValue::String(selection.kind.clone()),
        ),
        (
            "size".to_owned(),
            ConfigValue::Integer(i64::from(selection.size)),
        ),
    ]))]
}

/// One scenario location, as `game.places()` returns it: the fields and meanings of `game_select`'s (without `seq`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) size: u8,
}

/// The scenario's locations in file order, with the kind's id and the authored size, exactly as the map reports them to
/// `game_select`.
pub(crate) fn places_of(scenario: &ck_map::Scenario) -> Vec<Place> {
    scenario
        .locations
        .iter()
        .map(|location| Place {
            id: location.id.clone(),
            name: location.name.clone(),
            kind: scenario.location_kinds.kinds[location.kind_index]
                .id
                .clone(),
            size: location.size,
        })
        .collect()
}

/// `game.places()`: the scenario's locations in file order, as an array of `{id, name, kind, size}`. It is read-only scenario
/// data (no simulation state, no new delivery); it takes no arguments.
pub(crate) fn places_function(places: Vec<Place>) -> HostFunction {
    let table = ConfigValue::Array(
        places
            .into_iter()
            .map(|place| {
                ConfigValue::Table(BTreeMap::from([
                    ("id".to_owned(), ConfigValue::String(place.id)),
                    ("name".to_owned(), ConfigValue::String(place.name)),
                    ("kind".to_owned(), ConfigValue::String(place.kind)),
                    (
                        "size".to_owned(),
                        ConfigValue::Integer(i64::from(place.size)),
                    ),
                ]))
            })
            .collect(),
    );
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.places: takes no arguments".to_owned());
        }
        Ok(vec![table.clone()])
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_place_arguments_use_authored_kind_and_size_fields() {
        let arguments = select_arguments(&crate::map_app::Selection {
            seq: 7,
            id: "fixture-town".to_owned(),
            name: "Fixture Town".to_owned(),
            kind: "town".to_owned(),
            size: 3,
        });
        let [ConfigValue::Table(fields)] = arguments.as_slice() else {
            panic!("selection callback gets one table");
        };
        assert_eq!(fields.get("seq"), Some(&ConfigValue::Integer(7)));
        assert_eq!(
            fields.get("id"),
            Some(&ConfigValue::String("fixture-town".to_owned()))
        );
        assert_eq!(
            fields.get("name"),
            Some(&ConfigValue::String("Fixture Town".to_owned()))
        );
        assert_eq!(
            fields.get("kind"),
            Some(&ConfigValue::String("town".to_owned()))
        );
        assert_eq!(fields.get("size"), Some(&ConfigValue::Integer(3)));
    }

    /// `game.places()` over the shipped Ireland 768 scenario: its 94 locations in file order, the first and last literal, each with the
    /// kind's id and the authored size that `game_select` reports for the same location.
    #[test]
    fn places_returns_ireland_768s_locations_in_file_order_with_the_select_fields() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the shipped scenario loads");
        let places = places_of(&scenario);
        assert_eq!(places.len(), 94);
        assert_eq!(
            places[0],
            Place {
                id: "armagh".to_owned(),
                name: "Ard Mhacha".to_owned(),
                kind: "monastery".to_owned(),
                size: 4,
            }
        );
        assert_eq!(
            places[93],
            Place {
                id: "moynagh".to_owned(),
                name: "Maigh nEach".to_owned(),
                kind: "crannog".to_owned(),
                size: 1,
            }
        );
        // File order, and the fields are the ones a selection of the same location carries.
        for (place, location) in places.iter().zip(&scenario.locations) {
            assert_eq!(place.id, location.id);
            assert_eq!(place.name, location.name);
            assert_eq!(place.size, location.size);
            assert_eq!(
                place.kind,
                scenario.location_kinds.kinds[location.kind_index].id
            );
            let selected = select_arguments(&crate::map_app::Selection {
                seq: 1,
                id: place.id.clone(),
                name: place.name.clone(),
                kind: place.kind.clone(),
                size: place.size,
            });
            let ConfigValue::Table(selected) = &selected[0] else {
                panic!("a selection is a table");
            };
            assert_eq!(
                selected.get("kind"),
                Some(&ConfigValue::String(place.kind.clone()))
            );
            assert_eq!(
                selected.get("size"),
                Some(&ConfigValue::Integer(i64::from(place.size)))
            );
        }
        // The host function returns them as one array of `{id, name, kind, size}` and takes no argument.
        let function = places_function(places.clone());
        let Ok(returned) = function(&[]) else {
            panic!("game.places() with no arguments succeeds");
        };
        let [ConfigValue::Array(rows)] = returned.as_slice() else {
            panic!("one array");
        };
        assert_eq!(rows.len(), 94);
        let ConfigValue::Table(first) = &rows[0] else {
            panic!("a table per place");
        };
        assert_eq!(
            first.keys().map(String::as_str).collect::<Vec<_>>(),
            ["id", "kind", "name", "size"]
        );
        assert_eq!(first["name"], ConfigValue::String("Ard Mhacha".to_owned()));
        let refused = function(&[Some(ConfigValue::Integer(1))]).unwrap_err();
        assert_eq!(refused, "game.places: takes no arguments");
    }
}
