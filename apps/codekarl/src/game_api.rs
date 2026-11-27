//! The `game` extension of the UI VM: how the simulation reaches the UI.
//!
//! `game.on_time(fn | nil)` registers the callback `game_time` (and exists only when the configuration has `[game] scenario`).
//! The UI link posts it with the latest [`SimSnapshot`] as a table, `{year, month, day, hour, speed, paused, economy_revision}`,
//! where `paused` is `false` or the reason as the string `"player"`, `"error"` or `"load"`: see `ui_link` for when.
//! Its `weather` table maps place ids to the current hour's temperature in tenths and optional wind in knots and compass direction.
//!
//! `game.place(id, revision)` returns the economy view of a place at a revision a `game_time` carried (milestone 9, design §6). It
//! reads only the [`ViewCache`] the UI link fills when it posts a snapshot, never the live simulation.
//!
//! `game.characters(revision)` returns the cards of the live characters at that revision, in id order, and `game.character(id, revision)` the
//! first-tab record of one (characters slice 9a): both read the same [`ViewCache`] and refuse a missing or retired revision, and an id that is
//! not live there, by name. The `game_time` table also carries `world`: `0` for the session's first world, one more each time a load replaces the
//! simulation, never falling, so the UI sees a load even when the snapshot it published was replaced before it was read.
//!
//! `game.character_relations(id, revision)` returns the relations record of one live character at that revision (characters slice 9b): its
//! parents, spouses, children and others, the strands and opinion of each, and its passions. It reads the revision's own world (the live
//! characters and the archive as of that publication) and nothing else, and refuses as `game.character` does.
//!
//! `game.character_pop(id, revision)` returns the exact pop of a live character at that revision (place and slot) with the live characters of that pop,
//! most important first (the pop tab, slice 9c), and `game.character_log(id, revision)` the character's life log as of that revision's publication
//! (the life log window): both refuse as `game.character` does.
//!
//! `game.mark_unsaved()` and `game.load_save_guarded(filename)` share the simulation's ordered message route with saves and loads. The
//! guarded load returns immediately; its eventual outcome reaches `game_save_result(filename, result)` after
//! `game.on_save_result(fn | nil)` registers a callback.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ck_lua::{ConfigValue, Extension, HostFunction};

use crate::game::character_relations::{End, PassionLine, Person, Relations};
use crate::game::character_view::{CharacterCard, FirstTab};
use crate::game::controls::{self, GuardedLoadResult, SimControls};
use crate::game::core::{SaveCall, SaveReply};
use crate::game::saves::Entry;
use crate::game::slot::{PauseReason, SimSnapshot};
use crate::game::view::{BuildingView, EconomyView, PlaceView, PopView};
use crate::place_names::PlaceNameSetting;
use crate::vm::UI_CALL_LIMIT;
use ck_economy::genetics::{ATTRIBUTES, HUMOURS};
use ck_economy::personality::{MANNER_PAIRS, VICE_PAIRS};

/// The callback `game.on_time` registers.
pub(crate) const GAME_TIME: &str = "game_time";
/// The callback `game.on_hover` registers.
pub(crate) const GAME_HOVER: &str = "game_hover";
/// The callback `game.on_select` registers.
pub(crate) const GAME_SELECT: &str = "game_select";
/// The callback for the completed outcome of a guarded load.
pub(crate) const GAME_SAVE_RESULT: &str = "game_save_result";

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
    let result_controls = controls.clone();
    Extension::new("game")
        .function("control", control_function(controls.clone()))
        .function("pause", pause_function(controls.clone()))
        .function("set_modal", set_modal_function(controls.clone()))
        .function(
            "set_find_keyboard_block",
            set_find_keyboard_block_function(controls.clone()),
        )
        .function("set_map_layer", set_map_layer_function(controls.clone()))
        .function(
            "set_log_choices",
            set_log_choices_function(controls.clone()),
        )
        .function("mark_unsaved", mark_unsaved_function(controls.clone()))
        .function("saves", saves_function(controls.clone()))
        .function("save", save_function(controls.clone(), SaveCall::New))
        .function("save_over", save_function(controls.clone(), SaveCall::Over))
        .function(
            "delete_save",
            save_function(controls.clone(), SaveCall::Delete),
        )
        .function("load_save", save_function(controls.clone(), SaveCall::Load))
        .function(
            "history_subjects",
            history_subjects_function(controls.history().clone()),
        )
        .function(
            "history_graph",
            history_graph_function(controls.history().clone()),
        )
        .function(
            "history_take",
            history_take_function(controls.history().clone()),
        )
        .function("load_save_guarded", guarded_load_function(controls))
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
        .callback_observed(
            "on_save_result",
            GAME_SAVE_RESULT,
            Arc::new(move |is_registered| {
                result_controls.set_save_result_registered(is_registered);
            }),
        )
}

fn set_log_choices_function(controls: SimControls) -> HostFunction {
    Arc::new(move |args| {
        let [
            Some(ConfigValue::Bool(storms)),
            Some(ConfigValue::Integer(threshold)),
            Some(ConfigValue::Bool(blights)),
            Some(ConfigValue::String(sort)),
            Some(ConfigValue::Bool(descending)),
        ] = args
        else {
            return Err(
                "game.set_log_choices: expected storms, threshold, blights, sort and direction"
                    .into(),
            );
        };
        let threshold = u8::try_from(*threshold)
            .ok()
            .filter(|n| *n <= 64)
            .ok_or("game.set_log_choices: threshold must be 0..64")?;
        let sort = match sort.as_str() {
            "default" if !descending => ck_sim::LogSort::Default,
            "time" => ck_sim::LogSort::Time,
            "strength" => ck_sim::LogSort::Strength,
            _ => return Err("game.set_log_choices: invalid sort or direction".into()),
        };
        controls.set_log_choices(ck_sim::LogChoices {
            storms: *storms,
            threshold,
            blights: *blights,
            sort,
            descending: *descending,
        });
        Ok(Vec::new())
    })
}

fn set_map_layer_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(name)),
            Some(ConfigValue::Bool(shown)),
        ] = arguments
        else {
            return Err(
                "game.set_map_layer: expected a layer name (rivers, roads or expeditions) and a boolean"
                    .to_owned(),
            );
        };
        let layer = match name.as_str() {
            "rivers" => crate::game::slot::MapLayer::Rivers,
            "roads" => crate::game::slot::MapLayer::Roads,
            "expeditions" => crate::game::slot::MapLayer::Expeditions,
            _ => {
                return Err(
                    "game.set_map_layer: expected a layer name (rivers, roads or expeditions) and a boolean"
                        .to_owned(),
                );
            }
        };
        controls.set_map_layer(layer, *shown);
        Ok(Vec::new())
    })
}

fn mark_unsaved_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.mark_unsaved: expected no arguments".to_owned());
        }
        controls.mark_unsaved();
        Ok(Vec::new())
    })
}

/// `game.history_subjects()`: the tags, places, goods and pops the history viewer can graph (CK-441, rules 25 and 42). `nil` while
/// the game has no history.
fn history_subjects_function(host: crate::game::history_view::Host) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.history_subjects: expected no arguments".to_owned());
        }
        Ok(host.subjects().into_iter().collect())
    })
}

fn history_field<'a>(
    fields: &'a BTreeMap<String, ConfigValue>,
    key: &str,
) -> Result<&'a ConfigValue, String> {
    fields
        .get(key)
        .ok_or_else(|| format!("game.history_graph: missing {key}"))
}

fn history_text<'a>(
    fields: &'a BTreeMap<String, ConfigValue>,
    key: &str,
) -> Result<&'a str, String> {
    match history_field(fields, key)? {
        ConfigValue::String(value) => Ok(value),
        _ => Err(format!("game.history_graph: {key} must be text")),
    }
}

fn history_request(
    arguments: &[Option<ConfigValue>],
) -> Result<crate::game::history_view::Request, String> {
    use crate::game::history_view::{Graph, Range, Request, Subject};
    let [Some(ConfigValue::Table(fields))] = arguments else {
        return Err("game.history_graph: expected one table".to_owned());
    };
    let ConfigValue::Table(subject) = history_field(fields, "subject")? else {
        return Err("game.history_graph: subject must be a table".to_owned());
    };
    let id = history_text(subject, "id").ok();
    let subject =
        match (history_text(subject, "kind")?, id) {
            ("global", _) => Subject::Global,
            ("tag", Some(id)) => Subject::Tag(id.to_owned()),
            ("place", Some(id)) => Subject::Place(id.to_owned()),
            ("pop", Some(id)) => Subject::Pop(u64::from_str_radix(id, 16).map_err(|_| {
                "game.history_graph: a pop's id is 16 hexadecimal digits".to_owned()
            })?),
            _ => return Err("game.history_graph: unknown subject".to_owned()),
        };
    let graph = Graph::parse(history_text(fields, "graph")?)
        .ok_or_else(|| "game.history_graph: unknown graph".to_owned())?;
    let range = Range::parse(history_text(fields, "range")?)
        .ok_or_else(|| "game.history_graph: unknown range".to_owned())?;
    let number = |key: &str| match history_field(fields, key)? {
        ConfigValue::Integer(value) if *value >= 0 => {
            u64::try_from(*value).map_err(|_| format!("game.history_graph: {key} is too large"))
        }
        _ => Err(format!("game.history_graph: {key} must be a whole number")),
    };
    Ok(Request {
        subject,
        graph,
        range,
        budget: u32::try_from(number("budget")?).unwrap_or(u32::MAX),
        serial: number("request")?,
    })
}

/// `game.history_graph{subject, graph, range, budget, request}`: asks for a graph. The answer is computed off the simulation thread
/// and comes back through `game.history_take()` carrying `request`, the history revision it was computed from and `budget`.
fn history_graph_function(host: crate::game::history_view::Host) -> HostFunction {
    Arc::new(move |arguments| {
        host.request(history_request(arguments)?);
        Ok(Vec::new())
    })
}

/// `game.history_take()`: the answers that are ready, oldest first.
fn history_take_function(host: crate::game::history_view::Host) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.history_take: expected no arguments".to_owned());
        }
        Ok(vec![ConfigValue::Array(
            host.take()
                .iter()
                .map(crate::game::history_view::Answer::to_value)
                .collect(),
        )])
    })
}

fn guarded_load_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::String(filename))] = arguments else {
            return Err("game.load_save_guarded: expected one save filename".to_owned());
        };
        controls.guarded_load(filename.clone());
        Ok(Vec::new())
    })
}

pub(crate) fn guarded_result_arguments(result: &GuardedLoadResult) -> Vec<ConfigValue> {
    vec![
        ConfigValue::String(result.filename.clone()),
        result_value(result.result.clone()),
    ]
}

fn saves_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.saves: expected no arguments".to_owned());
        }
        let result = controls
            .save_call(SaveCall::List)
            .and_then(|reply| match reply {
                SaveReply::Listed(result) => result,
                SaveReply::Finished(_) => Err("unexpected save reply".to_owned()),
            });
        Ok(vec![match result {
            Ok(entries) => ConfigValue::Table(BTreeMap::from([
                ("ok".to_owned(), ConfigValue::Bool(true)),
                (
                    "entries".to_owned(),
                    ConfigValue::Array(entries.iter().map(entry_value).collect()),
                ),
            ])),
            Err(reason) => error_value(reason),
        }])
    })
}

fn save_function(controls: SimControls, build: fn(String) -> SaveCall) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::String(filename))] = arguments else {
            return Err("save call: expected one string argument".to_owned());
        };
        let result = controls
            .save_call(build(filename.clone()))
            .and_then(|reply| match reply {
                SaveReply::Finished(result) => result,
                SaveReply::Listed(_) => Err("unexpected save reply".to_owned()),
            });
        Ok(vec![result_value(result)])
    })
}

fn result_value(result: Result<(), String>) -> ConfigValue {
    match result {
        Ok(()) => ConfigValue::Table(BTreeMap::from([("ok".to_owned(), ConfigValue::Bool(true))])),
        Err(reason) => error_value(reason),
    }
}

fn error_value(reason: String) -> ConfigValue {
    ConfigValue::Table(BTreeMap::from([
        ("ok".to_owned(), ConfigValue::Bool(false)),
        ("reason".to_owned(), ConfigValue::String(reason)),
    ]))
}

fn entry_value(entry: &Entry) -> ConfigValue {
    let mut fields = BTreeMap::from([
        (
            "filename".to_owned(),
            ConfigValue::String(entry.filename.clone()),
        ),
        ("name".to_owned(), ConfigValue::String(entry.name.clone())),
        (
            "fit".to_owned(),
            ConfigValue::String(entry.fit.as_str().to_owned()),
        ),
        (
            "this_build".to_owned(),
            ConfigValue::String(
                match entry.this_build {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "unknown",
                }
                .to_owned(),
            ),
        ),
    ]);
    if let Some(written) = &entry.written {
        fields.insert("written".to_owned(), ConfigValue::String(written.clone()));
    }
    if let Some(date) = &entry.date {
        fields.insert("date".to_owned(), ConfigValue::String(date.clone()));
    }
    ConfigValue::Table(fields)
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

/// `game.pause()` sends the pause-only control: a running game pauses with the reason `player`, a paused one is left as it is. It is not
/// `time_pause`, which toggles from the simulation's own state, so it is right even when the UI's last `game_time` is out of date.
fn pause_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.pause: expected no arguments".to_owned());
        }
        controls.send_control(controls::Control::Pause);
        Ok(Vec::new())
    })
}

/// `game.set_modal(on)` tells the host a modal menu is open (or closed): the game's own key actions (`controls::BLOCKED_ACTIONS`) stop
/// while it is, and a gesture the map or a widget had captured before the block turned on ends, and every other action still arrives.
fn set_modal_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::Bool(on))] = arguments else {
            return Err("game.set_modal: expected one boolean argument (on)".to_owned());
        };
        controls.set_keys_blocked(*on);
        Ok(Vec::new())
    })
}

fn set_find_keyboard_block_function(controls: SimControls) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::Bool(on))] = arguments else {
            return Err("game.set_find_keyboard_block: expected one boolean".to_owned());
        };
        controls.set_find_keyboard_block(*on);
        Ok(Vec::new())
    })
}

/// The callback argument for a hovered marker; `None` means Lua receives no arguments (`nil`).
///
/// The positions are the map's, in window points; in a window opened in pixels `units` converts them to the UI's units at 1/256 precision.
pub(crate) fn hover_arguments(
    hover: Option<&crate::map_app::Hover>,
    units: Option<ck_render_2d::ui_scale::UnitMap>,
) -> Vec<ConfigValue> {
    let Some(hover) = hover else {
        return Vec::new();
    };
    // Whole points in a window opened in points; in one opened in pixels the same 1/256-unit conversion as a pointer position, which the
    // script sees as a number (the fixed value over 256), as it does a pointer's.
    let number = |points: i64, horizontal: bool| match units {
        None => ConfigValue::Integer(points),
        Some(map) => {
            let fixed = if horizontal {
                map.x_fixed(points)
            } else {
                map.y_fixed(points)
            };
            ConfigValue::Float(f64::from(fixed) / 256.0)
        }
    };
    vec![ConfigValue::Table(BTreeMap::from([
        ("id".to_owned(), ConfigValue::String(hover.id.clone())),
        ("name".to_owned(), ConfigValue::String(hover.name.clone())),
        ("left".to_owned(), number(hover.left, true)),
        ("right".to_owned(), number(hover.right, true)),
        ("y".to_owned(), number(hover.y, false)),
    ]))]
}

/// The argument for a selected place. Selection callbacks have no nil notification.
pub(crate) fn select_arguments(selection: &crate::map_app::Selection) -> Vec<ConfigValue> {
    let mut fields = BTreeMap::from([
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
    ]);
    if selection.abandoned {
        fields.insert(
            "state".to_owned(),
            ConfigValue::String("abandoned".to_owned()),
        );
    }
    vec![ConfigValue::Table(fields)]
}

/// One scenario location, as `game.places()` returns it: the fields and meanings of `game_select`'s (without `seq`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) id: String,
    /// The local (native) name.
    pub(crate) name: String,
    /// The English name, when the place has an established one (CK-423).
    pub(crate) name_en: Option<String>,
    pub(crate) kind: String,
    pub(crate) size: u8,
    pub(crate) abandoned: bool,
}

impl Place {
    /// The name the player's choice shows for this place.
    fn shown<'a>(&'a self, names: &PlaceNameSetting) -> &'a str {
        names.shown(&self.name, self.name_en.as_deref())
    }
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
            name_en: location.name_en.clone(),
            kind: scenario.location_kinds.kinds[location.kind_index]
                .id
                .clone(),
            size: location.size,
            abandoned: location.abandoned,
        })
        .collect()
}

/// `game.places()`: the scenario's locations in file order, as an array of `{id, name, local_name, english_name, kind, size}`. `name`
/// is the name the player's choice shows (CK-423): under English the place's English name, or its local one when it has none, and
/// under local its local name; `local_name` is always the local name and `english_name` is present only when the place has one. It is read-only scenario
/// data (no simulation state, no new delivery); it takes no arguments and reads the choice at each call.
pub(crate) fn places_function(places: Vec<Place>, names: PlaceNameSetting) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.places: takes no arguments".to_owned());
        }
        let rows = places
            .iter()
            .map(|place| {
                let mut fields = BTreeMap::from([
                    ("id".to_owned(), ConfigValue::String(place.id.clone())),
                    (
                        "name".to_owned(),
                        ConfigValue::String(place.shown(&names).to_owned()),
                    ),
                    (
                        "local_name".to_owned(),
                        ConfigValue::String(place.name.clone()),
                    ),
                    ("kind".to_owned(), ConfigValue::String(place.kind.clone())),
                    (
                        "size".to_owned(),
                        ConfigValue::Integer(i64::from(place.size)),
                    ),
                ]);
                if let Some(english) = &place.name_en {
                    fields.insert(
                        "english_name".to_owned(),
                        ConfigValue::String(english.clone()),
                    );
                }
                if place.abandoned {
                    fields.insert(
                        "state".to_owned(),
                        ConfigValue::String("abandoned".to_owned()),
                    );
                }
                ConfigValue::Table(fields)
            })
            .collect();
        Ok(vec![ConfigValue::Array(rows)])
    })
}

/// Searchable scenario names and live characters at the UI's world-bound revision.
pub(crate) fn find_entries_function(
    cache: Arc<ViewCache>,
    places: Vec<Place>,
    rivers: Vec<(String, String)>,
    names: PlaceNameSetting,
) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::Integer(revision))] = arguments else {
            return Err("game.find_entries: expected a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.find_entries: revision {revision} is missing"))?;
        let view = cache.get_in_world("game.find_entries", revision)?;
        let mut rows = Vec::new();
        for place in &places {
            let mut row = table([
                ("kind", text("location")),
                ("id", text(&place.id)),
                ("name", text(place.shown(&names))),
            ]);
            // The name the choice does not show, so a search matches both whichever is shown (CK-423).
            if let (ConfigValue::Table(fields), Some(other)) =
                (&mut row, names.other(&place.name, place.name_en.as_deref()))
            {
                fields.insert("other".to_owned(), text(other));
            }
            rows.push(row);
        }
        for card in &view.characters.cards {
            let name = if card.epithet.is_empty() {
                card.name.clone()
            } else {
                format!("{} {}", card.name, card.epithet)
            };
            let place = places
                .iter()
                .find(|p| p.id == card.place)
                .map_or("", |p| p.shown(&names));
            rows.push(table([
                ("kind", text("character")),
                ("id", integer(card.id)),
                ("name", text(&name)),
                ("place", text(place)),
            ]));
        }
        for (id, name) in &rivers {
            rows.push(table([
                ("kind", text("river")),
                ("id", text(id)),
                ("name", text(name)),
            ]));
        }
        Ok(vec![ConfigValue::Array(rows)])
    })
}

/// `game.scenario()`: the immutable id of the scenario for this run. It takes no arguments.
pub(crate) fn scenario_function(id: String) -> HostFunction {
    Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("game.scenario: takes no arguments".to_owned());
        }
        Ok(vec![ConfigValue::String(id.clone())])
    })
}

/// `game.set_map_mode(name)`: shows the map mode `"stockpile"`, `"yield"`, `"elevation"` or `"fertility"`, or the plain map for `"off"`. The values of the first two come
/// from the newest view the UI has been given; the elevation and fertility modes need none. An unknown name, or a map the lane refuses a table for, is an error.
pub(crate) fn set_map_mode_function(modes: Arc<crate::map_modes::MapModes>) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::String(name))] = arguments else {
            return Err("game.set_map_mode: expected one string argument (name)".to_owned());
        };
        let Some(mode) = crate::map_modes::Mode::parse(name) else {
            return Err(format!(
                "game.set_map_mode: unknown mode `{name}` (expected off, stockpile, yield, elevation or fertility)"
            ));
        };
        if let Err(message) = modes.set(mode) {
            let _ = ck_log::try_log!(ck_log::Level::Warn, "map", "{message}");
            return Err(format!("game.set_map_mode: {message}"));
        }
        Ok(Vec::new())
    })
}

/// The immutable event rows belonging to a delivered revision.
pub(crate) fn events_function(
    cache: Arc<ViewCache>,
    places: Vec<Place>,
    names: PlaceNameSetting,
) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::Integer(revision))] = arguments else {
            return Err("game.events: expected a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.events: revision {revision} is missing"))?;
        let rows = cache.get_events(revision)?;
        Ok(vec![ConfigValue::Array(
            rows.iter()
                .map(|row| {
                    use crate::game::events::{Action, EventKind};
                    let date = row.at.date().expect("validated event date");
                    let mut fields = BTreeMap::from([
                        ("year".into(), ConfigValue::Integer(i64::from(date.year))),
                        ("month".into(), ConfigValue::Integer(i64::from(date.month))),
                        ("day".into(), ConfigValue::Integer(i64::from(date.day))),
                        ("hour".into(), ConfigValue::Integer(i64::from(date.hour))),
                        (
                            "action".into(),
                            ConfigValue::String(
                                match row.action {
                                    Action::Begins => "begins",
                                    Action::Ends => "ends",
                                    Action::Merged => "merged",
                                }
                                .into(),
                            ),
                        ),
                    ]);
                    match &row.kind {
                        EventKind::Storm {
                            number,
                            snow,
                            cell,
                            knots,
                        } => {
                            fields.insert("kind".into(), ConfigValue::String("storm".into()));
                            fields.insert("number".into(), integer(*number));
                            fields.insert("snow".into(), ConfigValue::Bool(*snow));
                            fields.insert("knots".into(), ConfigValue::Integer(i64::from(*knots)));
                            fields.insert(
                                "location".into(),
                                ConfigValue::String(event_location(*cell)),
                            );
                        }
                        EventKind::Blight { good, place, .. } => {
                            fields.insert("kind".into(), ConfigValue::String("blight".into()));
                            fields.insert(
                                "good".into(),
                                ConfigValue::String(crate::game::events::good_name(good)),
                            );
                            let name = places
                                .iter()
                                .find(|p| p.id == *place)
                                .map_or(place.as_str(), |p| p.shown(&names));
                            fields.insert("place".into(), ConfigValue::String(name.to_owned()));
                            fields.insert("place_id".into(), ConfigValue::String(place.clone()));
                        }
                    }
                    ConfigValue::Table(fields)
                })
                .collect(),
        )])
    })
}

/// Active blighted goods at a place in a delivered event snapshot.
pub(crate) fn blights_function(cache: Arc<ViewCache>) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(place)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err("game.blights: expected a place and revision".to_owned());
        };
        let revision =
            u64::try_from(*revision).map_err(|_| "game.blights: invalid revision".to_owned())?;
        let rows = cache.get_events(revision)?;
        let mut active = BTreeMap::<String, usize>::new();
        for row in rows.iter() {
            if let crate::game::events::EventKind::Blight {
                good,
                place: event_place,
                ..
            } = &row.kind
                && event_place == place
            {
                let count = active.entry(good.clone()).or_default();
                match row.action {
                    crate::game::events::Action::Begins => *count += 1,
                    crate::game::events::Action::Ends => *count = count.saturating_sub(1),
                    crate::game::events::Action::Merged => {}
                }
            }
        }
        Ok(vec![ConfigValue::Array(
            active
                .into_iter()
                .filter(|(_, count)| *count > 0)
                .map(|(good, _)| ConfigValue::String(good))
                .collect(),
        )])
    })
}

fn event_location(cell: [u32; 2]) -> String {
    let radius = ck_map::MERCATOR_RADIUS_METRES;
    let half = std::f64::consts::PI * radius;
    let x = (f64::from(cell[0]) + 0.5) * ck_map::CELL_METRES - half;
    let y = half - (f64::from(cell[1]) + 0.5) * ck_map::CELL_METRES;
    let lon = (x / radius).to_degrees().round();
    let lat = (2.0 * (y / radius).exp().atan() - std::f64::consts::FRAC_PI_2)
        .to_degrees()
        .round();
    format!(
        "{:.0}°{} {:.0}°{}",
        lat.abs(),
        if lat < 0.0 { "S" } else { "N" },
        lon.abs(),
        if lon < 0.0 { "W" } else { "E" }
    )
}

/// How many views the cache can hold at most: the views of the `game_time` calls the UI link may have outstanding (at most
/// [`UI_CALL_LIMIT`] less the place reserved for lifecycle calls) and the one most recently completed. It is the bound, not the rule
/// by which views leave (see [`ViewCache`]).
pub(crate) const VIEW_CACHE_CAPACITY: usize = UI_CALL_LIMIT + 1;

#[derive(Default)]
struct CacheInner {
    /// The views held, by revision.
    views: BTreeMap<u64, Arc<EconomyView>>,
    /// For each revision, how many posted `game_time` calls that carry it have not completed. Several calls can carry one revision
    /// (a snapshot per hour reuses the day's view), and each pins it.
    pins: BTreeMap<u64, usize>,
    /// The revision of the `game_time` call that completed last.
    completed: Option<u64>,
    /// The highest revision that has left the cache: a revision at or below it that is not held is retired.
    retired_through: u64,
    /// The world of each held revision the link has noted (CK-381); a revision with none is of world `0`.
    worlds: BTreeMap<u64, u64>,
    /// The highest world noted: the world a go-to may name a revision of.
    newest_world: u64,
}

/// The economy views the UI link has handed to the UI VM (design r5 §3.11, kept in §6.2). The cache holds, by revision, the view of
/// every posted `game_time` call that has not completed, plus the view of the one that completed most recently. A revision leaves it,
/// and is then refused by name, when a later `game_time` call completes. The most views it can hold is [`VIEW_CACHE_CAPACITY`].
///
/// The UI link (main thread) [`post`](Self::post)s a snapshot's view before it posts the call, [`unpost`](Self::unpost)s it if the post
/// failed and [`complete`](Self::complete)s it when it takes the call's result; `game.place` (UI VM thread) [`get`](Self::get)s.
#[derive(Default)]
pub(crate) struct ViewCache {
    inner: Mutex<CacheInner>,
    events: Mutex<EventCache>,
    /// Told of each view as it is posted, once (the map modes' values come from the newest view).
    observer: std::sync::OnceLock<ViewObserver>,
}

#[derive(Default)]
struct EventCache {
    rows: BTreeMap<u64, Arc<Vec<crate::game::events::Event>>>,
    pins: BTreeMap<u64, usize>,
    completed: Option<u64>,
    retired_through: u64,
}

impl EventCache {
    fn retire(&mut self) {
        let gone: Vec<_> = self
            .rows
            .keys()
            .copied()
            .filter(|r| !self.pins.contains_key(r) && Some(*r) != self.completed)
            .collect();
        for revision in gone {
            self.rows.remove(&revision);
            self.retired_through = self.retired_through.max(revision);
        }
    }
    fn unpin(&mut self, revision: u64) {
        if let Some(n) = self.pins.get_mut(&revision) {
            *n -= 1;
            if *n == 0 {
                self.pins.remove(&revision);
            }
        }
    }
}

/// What a view cache tells of a posted view: its revision and the view.
pub(crate) type ViewObserver = Arc<dyn Fn(u64, &Arc<EconomyView>) + Send + Sync>;

impl CacheInner {
    /// Drops every view that is neither pinned by a pending call nor the last completed one, noting the highest that left.
    fn retire_unpinned(&mut self) {
        let completed = self.completed;
        let pins = &self.pins;
        let gone: Vec<u64> = self
            .views
            .keys()
            .copied()
            .filter(|revision| !pins.contains_key(revision) && Some(*revision) != completed)
            .collect();
        for revision in gone {
            self.views.remove(&revision);
            self.worlds.remove(&revision);
            self.retired_through = self.retired_through.max(revision);
        }
    }
}

impl ViewCache {
    pub(crate) fn post_events(&self, revision: u64, rows: Arc<Vec<crate::game::events::Event>>) {
        let mut cache = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        cache.rows.entry(revision).or_insert(rows);
        *cache.pins.entry(revision).or_default() += 1;
    }
    pub(crate) fn unpost_events(&self, revision: u64) {
        let mut cache = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        cache.unpin(revision);
        cache.retire();
    }
    pub(crate) fn complete_events(&self, revision: u64) {
        let mut cache = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        cache.unpin(revision);
        cache.completed = cache.completed.max(Some(revision));
        cache.retire();
    }
    fn get_events(&self, revision: u64) -> Result<Arc<Vec<crate::game::events::Event>>, String> {
        let cache = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        cache.rows.get(&revision).cloned().ok_or_else(|| {
            let state = if revision != 0 && revision <= cache.retired_through {
                "retired"
            } else {
                "missing"
            };
            format!("game.events: revision {revision} is {state}")
        })
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `view` under `revision` for a `game_time` call about to be posted, and pins it until that call completes or is
    /// [`unpost`](Self::unpost)ed. A revision already held keeps its first view (a revision names one view).
    pub(crate) fn post(&self, revision: u64, view: Arc<EconomyView>) {
        let told = Arc::clone(&view);
        {
            let mut inner = self.locked();
            inner.views.entry(revision).or_insert(view);
            *inner.pins.entry(revision).or_default() += 1;
            debug_assert!(
                inner.views.len() <= VIEW_CACHE_CAPACITY,
                "the UI link posts at most the call limit's worth of calls, so the cache cannot outgrow its bound"
            );
        }
        // Outside the lock: the observer may publish to the map lane.
        if let Some(observer) = self.observer.get() {
            observer(revision, &told);
        }
    }

    /// Records that the view of `revision` belongs to `world` (the `game_time` snapshot's world) and that no world below it is current.
    pub(crate) fn note_world(&self, revision: u64, world: u64) {
        let mut inner = self.locked();
        if inner.views.contains_key(&revision) {
            inner.worlds.insert(revision, world);
        }
        inner.newest_world = inner.newest_world.max(world);
    }

    /// [`get_for`](Self::get_for), and a refusal when the revision belongs to a world a load has since replaced.
    pub(crate) fn get_in_world(
        &self,
        caller: &str,
        revision: u64,
    ) -> Result<Arc<EconomyView>, String> {
        let view = self.get_for(caller, revision)?;
        let inner = self.locked();
        let world = inner.worlds.get(&revision).copied().unwrap_or_default();
        if world < inner.newest_world {
            return Err(format!(
                "{caller}: revision {revision} is of a replaced world"
            ));
        }
        Ok(view)
    }

    /// Has `observer` told of every view posted from now on. Set once; a second call changes nothing.
    pub(crate) fn observe_with(&self, observer: ViewObserver) {
        let _ = self.observer.set(observer);
    }

    fn unpin(inner: &mut CacheInner, revision: u64) {
        if let Some(count) = inner.pins.get_mut(&revision) {
            *count -= 1;
            if *count == 0 {
                inner.pins.remove(&revision);
            }
        }
    }

    /// The post that [`post`](Self::post) announced failed: the call does not exist, so it pins nothing, and a view that only it
    /// held leaves the cache without having been served.
    pub(crate) fn unpost(&self, revision: u64) {
        let mut inner = self.locked();
        Self::unpin(&mut inner, revision);
        inner.retire_unpinned();
    }

    /// The `game_time` call that carried `revision` completed: it is now the most recently completed, and every revision no pending
    /// call pins that is not it leaves the cache.
    pub(crate) fn complete(&self, revision: u64) {
        let mut inner = self.locked();
        Self::unpin(&mut inner, revision);
        // Calls complete in the order they were posted, and revisions rise with them.
        inner.completed = inner.completed.max(Some(revision));
        inner.retire_unpinned();
    }

    /// The view of `revision`, or the refusal that names it: retired (it was held and has left) or missing (it was never given to
    /// the UI).
    pub(crate) fn get(&self, revision: u64) -> Result<Arc<EconomyView>, String> {
        self.get_for("game.place", revision)
    }

    /// [`get`](Self::get) for the function `caller`, whose name the refusal carries.
    pub(crate) fn get_for(&self, caller: &str, revision: u64) -> Result<Arc<EconomyView>, String> {
        let inner = self.locked();
        if let Some(view) = inner.views.get(&revision) {
            return Ok(Arc::clone(view));
        }
        // Revision 0 is "no view" and was never held, whatever has left since.
        if revision != 0 && revision <= inner.retired_through {
            Err(format!("{caller}: revision {revision} is retired"))
        } else {
            Err(format!("{caller}: revision {revision} is missing"))
        }
    }

    /// How many views are held, for tests of the bound.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.locked().views.len()
    }
}

fn integer(value: u64) -> ConfigValue {
    ConfigValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

fn integer_u128(value: u128) -> ConfigValue {
    ConfigValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

fn text(value: &str) -> ConfigValue {
    ConfigValue::String(value.to_owned())
}

fn table<const N: usize>(fields: [(&str, ConfigValue); N]) -> ConfigValue {
    ConfigValue::Table(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

fn terrain_table(values: &BTreeMap<String, u32>) -> ConfigValue {
    ConfigValue::Table(
        values
            .iter()
            .map(|(terrain, count)| (terrain.clone(), integer(u64::from(*count))))
            .collect(),
    )
}

fn counter_rows(rows: &[crate::game::view::GoodCounterView]) -> ConfigValue {
    ConfigValue::Array(
        rows.iter()
            .map(|row| {
                let mut fields = BTreeMap::from([
                    ("good".to_owned(), text(&row.good)),
                    ("units".to_owned(), integer(row.units)),
                ]);
                if let Some(value) = row.value_milli {
                    fields.insert("value_milli".to_owned(), integer_u128(value));
                }
                ConfigValue::Table(fields)
            })
            .collect(),
    )
}

fn stock_table(stock: &[crate::game::view::StockView]) -> ConfigValue {
    ConfigValue::Array(
        stock
            .iter()
            .map(|item| {
                table([
                    ("good", text(&item.good)),
                    ("amount", integer(item.amount)),
                    ("charged_ml", integer(item.charged_ml)),
                    ("outside", ConfigValue::Bool(item.outside)),
                ])
            })
            .collect(),
    )
}

fn pop_table(pop: &PopView) -> ConfigValue {
    table([
        ("stock", stock_table(&pop.stock)),
        ("room_ml", integer(pop.room_ml)),
        ("kind", text(&pop.kind)),
        ("culture", text(&pop.culture)),
        ("religion", text(&pop.religion)),
        ("workers", integer(pop.workers)),
        ("dependents", integer(pop.dependents)),
        ("employed", integer(pop.employed)),
        (
            "jobs",
            ConfigValue::Array(
                pop.jobs
                    .iter()
                    .map(|(building, job, workers)| {
                        table([
                            ("building", text(building)),
                            ("job", text(job)),
                            ("workers", integer(*workers)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("unemployed", integer(pop.unemployed)),
        ("food_days", integer(pop.food_days)),
        (
            "food",
            ConfigValue::Array(
                pop.food
                    .iter()
                    .map(|food| {
                        table([
                            ("good", text(&food.good)),
                            ("units", integer(food.units)),
                            ("unit", text(&food.unit)),
                            ("share_pm", integer(food.share_pm)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("necessities", integer(pop.necessities)),
        ("niceties", integer(pop.niceties)),
        ("luxuries", integer(pop.luxuries)),
        ("satisfaction", ConfigValue::Integer(pop.satisfaction)),
        ("equilibrium", ConfigValue::Integer(pop.equilibrium)),
        ("births", integer(pop.births)),
        ("deaths", integer(pop.deaths)),
        ("basic_housing", integer(pop.basic_housing)),
        ("makeshift_housing", integer(pop.makeshift_housing)),
        (
            "crowding_pm",
            pop.crowding_pm.map_or(ConfigValue::Bool(false), integer),
        ),
    ])
}

fn building_table(building: &BuildingView) -> ConfigValue {
    table([
        ("stock", stock_table(&building.stock)),
        (
            "made",
            ConfigValue::Array(
                building
                    .made
                    .iter()
                    .map(|(good, amount)| {
                        table([("good", text(good)), ("amount", integer(*amount))])
                    })
                    .collect(),
            ),
        ),
        ("store_capacity_ml", integer(building.store_capacity_ml)),
        ("kind", text(&building.kind)),
        ("levels", integer(building.levels)),
        ("has_methods", ConfigValue::Bool(building.has_methods)),
        ("units", integer(building.units)),
        ("free_units", integer(building.free_units)),
        (
            "methods",
            ConfigValue::Array(
                building
                    .methods
                    .iter()
                    .map(|(id, levels)| table([("id", text(id)), ("levels", integer(*levels))]))
                    .collect(),
            ),
        ),
        ("filled", integer(building.filled)),
        (
            "jobs",
            ConfigValue::Array(
                building
                    .jobs
                    .iter()
                    .map(|(id, filled, places)| {
                        table([
                            ("id", text(id)),
                            ("filled", integer(*filled)),
                            ("places", integer(*places)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("abandoned", ConfigValue::Bool(building.abandoned)),
        ("open", integer(building.open)),
        (
            "herds",
            ConfigValue::Array(
                building
                    .herds
                    .iter()
                    .map(|herd| {
                        table([
                            ("species", text(&herd.species)),
                            ("head", integer(herd.head)),
                            ("maximum", integer(herd.maximum)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("store_used_l", integer(building.store_used_ml / 1000)),
        (
            "store_capacity_l",
            integer(building.store_capacity_ml / 1000),
        ),
        ("wage", integer(building.wage)),
        (
            "wages",
            ConfigValue::Array(
                building
                    .wages
                    .iter()
                    .map(|wage| {
                        let mut fields = BTreeMap::from([
                            ("month".to_owned(), integer(u64::from(wage.month))),
                            ("wage_milli".to_owned(), integer(wage.wage_milli)),
                        ]);
                        if let Some(year) = wage.year {
                            fields.insert("year".to_owned(), integer(u64::from(year)));
                            fields
                                .insert("paid".to_owned(), integer(wage.paid.unwrap_or_default()));
                        }
                        ConfigValue::Table(fields)
                    })
                    .collect(),
            ),
        ),
        ("housing_occupants", integer(building.housing_occupants)),
    ])
}

/// A place's view as the table `game.place` returns. Volumes are whole litres, prices are in thousandths of a value unit.
pub(crate) fn place_table(place: &PlaceView) -> ConfigValue {
    let mut result = table([
        ("id", text(&place.id)),
        ("workers", integer(place.workers)),
        ("dependents", integer(place.dependents)),
        ("food_days", integer(place.food_days)),
        ("storage_used_l", integer(place.storage_used_ml / 1000)),
        (
            "storage_capacity_l",
            integer(place.storage_capacity_ml / 1000),
        ),
        ("overflow_lost", integer(place.overflow_lost)),
        (
            "land",
            table([
                ("usable", terrain_table(&place.land.usable)),
                ("land", terrain_table(&place.land.land)),
                ("sea", integer(u64::from(place.land.sea))),
            ]),
        ),
        (
            "last_month",
            table([
                ("closed", ConfigValue::Bool(place.last_month.closed)),
                ("produced", counter_rows(&place.last_month.produced)),
                ("used_by_pops", counter_rows(&place.last_month.used_by_pops)),
                ("lost", counter_rows(&place.last_month.lost)),
            ]),
        ),
        ("footprint_m2", integer(place.footprint_m2)),
        ("prepared_m2", integer(place.prepared_m2)),
        ("max_buildable_m2", integer(place.max_buildable_m2)),
        ("development_pm", integer(u64::from(place.development_pm))),
        (
            "farmland_quality_pm",
            integer(u64::from(place.farmland_quality_pm)),
        ),
        ("field_room", integer(place.field_room)),
        (
            "basic_housing_capacity",
            integer(place.basic_housing_capacity),
        ),
        (
            "basic_housing_occupants",
            integer(place.basic_housing_occupants),
        ),
        (
            "makeshift_housing_capacity",
            integer(place.makeshift_housing_capacity),
        ),
        (
            "makeshift_housing_occupants",
            integer(place.makeshift_housing_occupants),
        ),
        ("crowding_pm", integer(place.crowding_pm)),
        (
            "prices",
            ConfigValue::Array(
                place
                    .prices
                    .iter()
                    .map(|price| {
                        let mut fields = BTreeMap::from([
                            ("good".to_owned(), text(&price.good)),
                            (
                                "tags".to_owned(),
                                ConfigValue::Array(
                                    price.tags.iter().map(|tag| text(tag)).collect(),
                                ),
                            ),
                            ("price_milli".to_owned(), integer(price.price_milli)),
                            ("base".to_owned(), integer(price.base)),
                        ]);
                        if let Some(bought) = price.bought {
                            fields.insert("bought".to_owned(), integer_u128(bought));
                        }
                        if let Some(offered) = price.offered {
                            fields.insert("offered".to_owned(), integer_u128(offered));
                        }
                        ConfigValue::Table(fields)
                    })
                    .collect(),
            ),
        ),
        (
            "pops",
            ConfigValue::Array(place.pops.iter().map(pop_table).collect()),
        ),
        (
            "buildings",
            ConfigValue::Array(place.buildings.iter().map(building_table).collect()),
        ),
    ]);
    add_temperature(&mut result, place.temperature);
    add_wind(&mut result, place.wind.as_ref());
    result
}

fn add_wind(result: &mut ConfigValue, wind: Option<&(i64, String)>) {
    if let (Some((knots, direction)), ConfigValue::Table(fields)) = (wind, result) {
        fields.insert(
            "wind".to_owned(),
            table([
                ("knots", ConfigValue::Integer(*knots)),
                ("direction", text(direction)),
            ]),
        );
    }
}

fn add_temperature(result: &mut ConfigValue, temperature: Option<(i64, u8, i64)>) {
    if let (Some((mean, month, expected)), ConfigValue::Table(fields)) = (temperature, result) {
        fields.insert(
            "temperature".to_owned(),
            table([
                ("mean", ConfigValue::Integer(mean)),
                ("month", ConfigValue::Integer(i64::from(month))),
                ("expected", ConfigValue::Integer(expected)),
            ]),
        );
    }
}

/// `game.place(id, revision)`: the economy view of the place `id` at `revision`, from the cache and never from live state. It
/// refuses, by name, a revision that is missing or retired, a place the view does not hold and arguments of the wrong kind.
pub(crate) fn place_function(
    cache: Arc<ViewCache>,
    places: Vec<Place>,
    names: PlaceNameSetting,
) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(id)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err("game.place: expected a place id and a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.place: revision {revision} is missing"))?;
        let view = cache.get(revision)?;
        let place = view
            .place(id)
            .ok_or_else(|| format!("game.place: no place `{id}` at revision {revision}"))?;
        let mut result = place_table(place);
        if let Some(meta) = places
            .iter()
            .find(|candidate| candidate.id == *id && candidate.abandoned)
            && let ConfigValue::Table(fields) = &mut result
        {
            fields.insert(
                "name".to_owned(),
                ConfigValue::String(meta.shown(&names).to_owned()),
            );
            fields.insert("kind".to_owned(), ConfigValue::String(meta.kind.clone()));
            fields.insert(
                "size".to_owned(),
                ConfigValue::Integer(i64::from(meta.size)),
            );
            fields.insert(
                "state".to_owned(),
                ConfigValue::String("abandoned".to_owned()),
            );
        }
        Ok(vec![result])
    })
}

/// The card table of the ledger's Characters tab (CK-375).
///
/// The role line names its places as the player's choice shows them (CK-423): both forms are in the view, and the choice is read here.
fn card_table(card: &CharacterCard, names: &PlaceNameSetting) -> ConfigValue {
    let role = if names.choice() == crate::place_names::LOCAL {
        &card.role
    } else {
        &card.role_english
    };
    table([
        ("id", integer(card.id)),
        ("name", text(&card.name)),
        ("epithet", text(&card.epithet)),
        ("sex", text(if card.female { "f" } else { "m" })),
        ("age", integer(u64::from(card.age))),
        ("house", text(&card.house)),
        ("role", text(role)),
        ("place", text(&card.place)),
        ("slot", integer(u64::from(card.slot))),
        ("kind", text(&card.kind)),
        ("ruler", ConfigValue::Bool(card.ruler)),
        ("clergy", ConfigValue::Bool(card.clergy)),
        ("child", ConfigValue::Bool(card.child)),
        ("importance", integer(u64::from(card.importance))),
    ])
}

fn signed(value: i64) -> ConfigValue {
    ConfigValue::Integer(value)
}

/// `{key = ..., value = ...}` for each of `keys` with its value, in order.
fn keyed<'a>(keys: impl Iterator<Item = &'a str>, values: &[i64]) -> ConfigValue {
    ConfigValue::Array(
        keys.zip(values)
            .map(|(key, value)| table([("key", text(key)), ("value", signed(*value))]))
            .collect(),
    )
}

/// The first tab's table of a character (CK-375 §6): its header (as its card) and the values of each section.
fn character_table(card: &CharacterCard, tab: &FirstTab, names: &PlaceNameSetting) -> ConfigValue {
    let ConfigValue::Table(mut fields) = card_table(card, names) else {
        unreachable!("a card is a table")
    };
    let skills: Vec<i64> = tab.skills.iter().map(|(_, value)| *value).collect();
    let mut put = |key: &str, value: ConfigValue| {
        fields.insert(key.to_owned(), value);
    };
    put("body", keyed(ATTRIBUTES.iter().copied(), &tab.body));
    put("humours", keyed(HUMOURS.iter().copied(), &tab.humours));
    put(
        "skills",
        keyed(tab.skills.iter().map(|(key, _)| key.as_str()), &skills),
    );
    put("virtue", keyed(VICE_PAIRS.iter().copied(), &tab.virtue));
    put("stress", signed(tab.stress));
    put("threshold", signed(tab.threshold));
    put("manner", keyed(MANNER_PAIRS.iter().copied(), &tab.manner));
    put(
        "standing",
        keyed(["popularity", "fame", "honour"].into_iter(), &tab.standing),
    );
    put(
        "traits",
        ConfigValue::Array(
            tab.traits
                .iter()
                .map(|chip| {
                    table([
                        ("key", text(&chip.key)),
                        ("words", text(&chip.words)),
                        ("attribute", ConfigValue::Bool(chip.attribute)),
                        ("letter", text(chip.letter)),
                    ])
                })
                .collect(),
        ),
    );
    // Injuries are not simulated yet: the screen shows "none".
    put("health", ConfigValue::Array(Vec::new()));
    ConfigValue::Table(fields)
}

/// `game.characters(revision)`: the cards of the live characters at `revision`, in id order, from the cache and never from live state.
pub(crate) fn characters_function(cache: Arc<ViewCache>, names: PlaceNameSetting) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::Integer(revision))] = arguments else {
            return Err("game.characters: expected a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.characters: revision {revision} is missing"))?;
        let view = cache.get_for("game.characters", revision)?;
        Ok(vec![ConfigValue::Array(
            view.characters
                .cards
                .iter()
                .map(|card| card_table(card, &names))
                .collect(),
        )])
    })
}

/// `game.character(id, revision)`: the first-tab record of the character `id` at `revision`. An unknown id (not live at that revision) and a
/// revision that is missing or retired are refusals the UI shows nowhere.
pub(crate) fn character_function(cache: Arc<ViewCache>, names: PlaceNameSetting) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::Integer(id)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err("game.character: expected a character id and a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.character: revision {revision} is missing"))?;
        let view = cache.get_for("game.character", revision)?;
        let found = u64::try_from(*id).ok().and_then(|id| {
            let card = view.characters.card(id)?;
            Some((card, view.characters.first_tab(id)?))
        });
        let (card, tab) = found
            .ok_or_else(|| format!("game.character: no character {id} at revision {revision}"))?;
        Ok(vec![character_table(card, tab, &names)])
    })
}

/// One strand line of a shown person.
fn line_table(line: &crate::game::character_relations::Line) -> ConfigValue {
    table([
        ("kind", text(line.kind)),
        ("strength", integer(u64::from(line.strength))),
        ("side", text(line.side.word())),
        ("history", ConfigValue::Bool(line.history)),
    ])
}

/// One person shown on the relations tab (CK-377 §2-§3).
fn person_table(person: &Person) -> ConfigValue {
    let ConfigValue::Table(mut fields) = table([
        ("id", integer(person.id)),
        ("name", text(&person.name)),
        ("epithet", text(&person.epithet)),
        ("sex", text(if person.female { "f" } else { "m" })),
        ("live", ConfigValue::Bool(person.live())),
        ("age", integer(u64::from(person.age))),
        ("relation", text(&person.relation)),
        ("tie", text(person.tie.word())),
        (
            "lines",
            ConfigValue::Array(person.lines.iter().map(line_table).collect()),
        ),
    ]) else {
        unreachable!("a table is a table")
    };
    let mut put = |key: &str, value: ConfigValue| {
        fields.insert(key.to_owned(), value);
    };
    match person.end {
        End::Alive => {}
        End::Died(date) => {
            put("ended", text("died"));
            put("year", integer(u64::from(date.year)));
            put("month", integer(u64::from(date.month)));
            put("day", integer(u64::from(date.day)));
        }
        End::DiedBefore(year) => {
            put("ended", text("before"));
            put("year", integer(u64::from(year)));
        }
        End::Abroad => put("ended", text("abroad")),
    }
    if let Some((mine, back)) = person.opinion {
        put("opinion", signed(mine));
        put("opinion_back", signed(back));
    }
    ConfigValue::Table(fields)
}

fn passion_table(passion: &PassionLine, names: &PlaceNameSetting) -> ConfigValue {
    let name = if names.choice() == crate::place_names::LOCAL {
        &passion.name
    } else {
        &passion.name_english
    };
    table([
        ("kind", text(passion.kind)),
        ("strength", integer(u64::from(passion.strength))),
        ("target_kind", text(passion.target_kind)),
        ("key", text(&passion.key)),
        ("name", text(name)),
    ])
}

/// The relations record of a character as the table `game.character_relations` returns.
fn relations_table(id: u64, record: &Relations, names: &PlaceNameSetting) -> ConfigValue {
    let people = |people: &[Person]| ConfigValue::Array(people.iter().map(person_table).collect());
    let ConfigValue::Table(mut fields) = table([
        ("id", integer(id)),
        (
            "spouse_word",
            text(if record.spouses_are_wives {
                "wives"
            } else {
                "husbands"
            }),
        ),
        ("spouses", people(&record.spouses)),
        ("children", people(&record.children)),
        ("others", people(&record.others)),
        (
            "passions",
            ConfigValue::Array(
                record
                    .passions
                    .iter()
                    .map(|passion| passion_table(passion, names))
                    .collect(),
            ),
        ),
    ]) else {
        unreachable!("a table is a table")
    };
    if let Some(father) = &record.parents[0] {
        fields.insert("father".to_owned(), person_table(father));
    }
    if let Some(mother) = &record.parents[1] {
        fields.insert("mother".to_owned(), person_table(mother));
    }
    ConfigValue::Table(fields)
}

/// `game.character_relations(id, revision)`: the relations record of the live character `id` at `revision`, built on request from what that
/// revision's view carries. An id not live at that revision, and a revision that is missing or retired, are refusals.
pub(crate) fn character_relations_function(
    cache: Arc<ViewCache>,
    names: PlaceNameSetting,
) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::Integer(id)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err(
                "game.character_relations: expected a character id and a revision".to_owned(),
            );
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.character_relations: revision {revision} is missing"))?;
        let view = cache.get_for("game.character_relations", revision)?;
        let id = u64::try_from(*id).map_err(|_| {
            format!("game.character_relations: no character {id} at revision {revision}")
        })?;
        let record = view.characters.relations_of(id).map_err(|_| {
            format!("game.character_relations: no character {id} at revision {revision}")
        })?;
        Ok(vec![relations_table(id, &record, &names)])
    })
}

/// `game.character_pop(id, revision)`: the pop the live character `id` belongs to at `revision` (its exact place and slot) and the live characters
/// of that pop at that revision, most important first, each as its card (CK-378 §1). An id not live at that revision, and a revision that is
/// missing or retired, are refusals.
pub(crate) fn character_pop_function(
    cache: Arc<ViewCache>,
    names: PlaceNameSetting,
) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::Integer(id)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err("game.character_pop: expected a character id and a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.character_pop: revision {revision} is missing"))?;
        let view = cache.get_for("game.character_pop", revision)?;
        let card = u64::try_from(*id)
            .ok()
            .and_then(|id| view.characters.card(id))
            .ok_or_else(|| {
                format!("game.character_pop: no character {id} at revision {revision}")
            })?;
        let members = view
            .characters
            .pop_members(&card.place, card.slot)
            .iter()
            .filter_map(|member| view.characters.card(*member))
            .map(|member| card_table(member, &names))
            .collect();
        Ok(vec![table([
            ("place", text(&card.place)),
            ("slot", integer(u64::from(card.slot))),
            ("characters", ConfigValue::Array(members)),
        ])])
    })
}

/// Cards of the members of an exact pop slot at a delivered revision.
pub(crate) fn pop_members_function(cache: Arc<ViewCache>, names: PlaceNameSetting) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(place)),
            Some(ConfigValue::Integer(slot)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err("game.pop_members: expected place, slot and revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| "game.pop_members: invalid revision".to_owned())?;
        let slot = u32::try_from(*slot).map_err(|_| "game.pop_members: invalid slot".to_owned())?;
        let view = cache.get_for("game.pop_members", revision)?;
        let members = view
            .characters
            .pop_members(place, slot)
            .iter()
            .filter_map(|id| view.characters.card(*id))
            .map(|card| card_table(card, &names))
            .collect();
        Ok(vec![ConfigValue::Array(members)])
    })
}

/// `game.character_log(id, revision)`: the life log of the live character `id` as of `revision`, a list of `{year, month, day, text}` in date order
/// with append order for equal dates (CK-378 §2). It reads the revision's own world and its published boundary, and nothing live; an id not live
/// at that revision, and a revision that is missing or retired, are refusals.
pub(crate) fn character_log_function(cache: Arc<ViewCache>) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::Integer(id)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err("game.character_log: expected a character id and a revision".to_owned());
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.character_log: revision {revision} is missing"))?;
        let view = cache.get_for("game.character_log", revision)?;
        let lines = u64::try_from(*id)
            .map_err(|_| "no character".to_owned())
            .and_then(|id| view.characters.log_of(id))
            .map_err(|why| format!("game.character_log: {why} for {id} at revision {revision}"))?;
        Ok(vec![ConfigValue::Array(
            lines
                .iter()
                .map(|(date, line)| {
                    table([
                        ("year", integer(u64::from(date.year))),
                        ("month", integer(u64::from(date.month))),
                        ("day", integer(u64::from(date.day))),
                        ("text", text(line)),
                    ])
                })
                .collect(),
        )])
    })
}

/// The table a `game_time` callback gets for `snapshot`.
fn pause_value(reason: Option<PauseReason>) -> ConfigValue {
    match reason {
        None => ConfigValue::Bool(false),
        Some(PauseReason::Player) => ConfigValue::String("player".to_owned()),
        Some(PauseReason::Error) => ConfigValue::String("error".to_owned()),
        Some(PauseReason::Load) => ConfigValue::String("load".to_owned()),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "One table of every field the UI reads of a snapshot; each field is one entry"
)]
pub(crate) fn time_table(snapshot: &SimSnapshot) -> ConfigValue {
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
        ("paused".to_owned(), pause_value(snapshot.paused)),
        ("unsaved".to_owned(), ConfigValue::Bool(snapshot.unsaved)),
        (
            "rivers".to_owned(),
            ConfigValue::Bool(snapshot.layers.rivers),
        ),
        ("roads".to_owned(), ConfigValue::Bool(snapshot.layers.roads)),
        (
            "expeditions".to_owned(),
            ConfigValue::Bool(snapshot.layers.expeditions),
        ),
        (
            "economy_revision".to_owned(),
            integer(snapshot.economy_revision),
        ),
        (
            "events_revision".to_owned(),
            integer(snapshot.events_revision),
        ),
        (
            "history_revision".to_owned(),
            integer(snapshot.history_revision),
        ),
        (
            "log_storms".to_owned(),
            ConfigValue::Bool(snapshot.log_choices.storms),
        ),
        (
            "log_threshold".to_owned(),
            ConfigValue::Integer(i64::from(snapshot.log_choices.threshold)),
        ),
        (
            "log_blights".to_owned(),
            ConfigValue::Bool(snapshot.log_choices.blights),
        ),
        (
            "log_sort".to_owned(),
            ConfigValue::String(
                match snapshot.log_choices.sort {
                    ck_sim::LogSort::Default => "default",
                    ck_sim::LogSort::Time => "time",
                    ck_sim::LogSort::Strength => "strength",
                }
                .into(),
            ),
        ),
        (
            "log_descending".to_owned(),
            ConfigValue::Bool(snapshot.log_choices.descending),
        ),
        (
            "weather".to_owned(),
            ConfigValue::Table(
                snapshot
                    .weather
                    .iter()
                    .map(|(id, (temperature, wind))| {
                        let mut fields = BTreeMap::from([(
                            "temperature".to_owned(),
                            ConfigValue::Integer(*temperature),
                        )]);
                        if let Some((knots, direction)) = wind {
                            fields.insert(
                                "wind".to_owned(),
                                ConfigValue::Table(BTreeMap::from([
                                    ("knots".to_owned(), ConfigValue::Integer(*knots)),
                                    (
                                        "direction".to_owned(),
                                        ConfigValue::String(direction.clone()),
                                    ),
                                ])),
                            );
                        }
                        (id.clone(), ConfigValue::Table(fields))
                    })
                    .collect(),
            ),
        ),
        ("world".to_owned(), integer(snapshot.world)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{core::Message, saves::Fit};

    #[test]
    fn event_cache_pins_each_pending_revision_and_retires_older_rows() {
        let cache = ViewCache::default();
        cache.post_events(1, Arc::new(Vec::new()));
        cache.post_events(2, Arc::new(Vec::new()));
        cache.complete_events(1);
        assert!(cache.get_events(1).is_ok());
        assert!(cache.get_events(2).is_ok());
        cache.complete_events(2);
        assert!(cache.get_events(1).unwrap_err().contains("retired"));
        assert!(cache.get_events(2).is_ok());
        assert!(cache.get_events(3).unwrap_err().contains("missing"));
    }

    #[test]
    fn event_cell_centres_use_projection_and_round_away_from_zero() {
        for (cell, expected) in [
            ([21361, 15777], "52°N 19°W"),
            ([24280, 13323], "62°N 3°E"),
            ([22821, 15777], "52°N 8°W"),
            ([22953, 15559], "53°N 7°W"),
            ([20300, 15559], "53°N 27°W"),
            ([22157, 15559], "53°N 13°W"),
            ([21892, 15336], "54°N 15°W"),
            ([20963, 15990], "51°N 22°W"),
            ([23883, 23883], "0°N 0°E"),
            ([23816, 23950], "1°S 0°E"),
            ([23815, 23951], "1°S 1°W"),
        ] {
            assert_eq!(event_location(cell), expected);
        }
    }

    #[test]
    fn set_log_choices_uses_the_ordered_control_channel_and_checks_arguments() {
        let (tx, rx) = std::sync::mpsc::channel();
        let set = set_log_choices_function(SimControls::new(tx));
        let args = [
            Some(ConfigValue::Bool(true)),
            Some(ConfigValue::Integer(40)),
            Some(ConfigValue::Bool(true)),
            Some(ConfigValue::String("strength".into())),
            Some(ConfigValue::Bool(true)),
        ];
        set(&args).unwrap();
        assert_eq!(
            rx.recv().unwrap(),
            Message::SetLogChoices(ck_sim::LogChoices {
                storms: true,
                threshold: 40,
                blights: true,
                sort: ck_sim::LogSort::Strength,
                descending: true
            })
        );
        let mut invalid = args;
        invalid[1] = Some(ConfigValue::Integer(65));
        assert!(set(&invalid).is_err());
    }

    #[test]
    fn set_map_layer_validates_and_sends_one_ordered_message() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let set = set_map_layer_function(SimControls::new(sender));
        for (name, layer) in [
            ("rivers", crate::game::slot::MapLayer::Rivers),
            ("roads", crate::game::slot::MapLayer::Roads),
            ("expeditions", crate::game::slot::MapLayer::Expeditions),
        ] {
            assert!(
                set(&[
                    Some(ConfigValue::String(name.into())),
                    Some(ConfigValue::Bool(true))
                ])
                .unwrap()
                .is_empty()
            );
            assert_eq!(receiver.recv().unwrap(), Message::SetMapLayer(layer, true));
        }
        let name = Some(ConfigValue::String("rivers".into()));
        for args in [
            vec![],
            vec![name.clone()],
            vec![name.clone(), Some(ConfigValue::Bool(true)), None],
            vec![name.clone(), Some(ConfigValue::String("yes".into()))],
            vec![
                Some(ConfigValue::String("sea".into())),
                Some(ConfigValue::Bool(true)),
            ],
        ] {
            let error = set(&args).unwrap_err();
            assert!(error.contains("game.set_map_layer") && error.contains("expected"));
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn selected_place_arguments_use_authored_kind_and_size_fields() {
        let arguments = select_arguments(&crate::map_app::Selection {
            seq: 7,
            id: "fixture-town".to_owned(),
            name: "Fixture Town".to_owned(),
            kind: "town".to_owned(),
            size: 3,
            abandoned: false,
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

    #[test]
    fn game_time_includes_the_saved_status_with_its_existing_fields() {
        let mut snapshot = crate::readout_tests::snapshot(3, 2, None);
        snapshot
            .weather
            .insert("birr".into(), (108, Some((2, "SW".into()))));
        let value = time_table(&snapshot);
        let ConfigValue::Table(fields) = value else {
            panic!("game_time gets one table");
        };
        let keys: Vec<_> = fields.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "day",
                "economy_revision",
                "events_revision",
                "expeditions",
                "history_revision",
                "hour",
                "log_blights",
                "log_descending",
                "log_sort",
                "log_storms",
                "log_threshold",
                "month",
                "paused",
                "rivers",
                "roads",
                "speed",
                "unsaved",
                "weather",
                "world",
                "year",
            ]
        );
        assert_eq!(fields.get("unsaved"), Some(&ConfigValue::Bool(false)));
        let Some(ConfigValue::Table(weather)) = fields.get("weather") else {
            panic!("weather table")
        };
        let Some(ConfigValue::Table(birr)) = weather.get("birr") else {
            panic!("Birr weather")
        };
        assert_eq!(birr.get("temperature"), Some(&ConfigValue::Integer(108)));
        let Some(ConfigValue::Table(wind)) = birr.get("wind") else {
            panic!("wind table")
        };
        assert_eq!(wind.get("knots"), Some(&ConfigValue::Integer(2)));
        assert_eq!(
            wind.get("direction"),
            Some(&ConfigValue::String("SW".into()))
        );
    }

    #[test]
    fn save_host_calls_return_only_after_the_owner_reports_the_completed_result() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let list = saves_function(SimControls::new(sender));
        let caller = std::thread::spawn(move || list(&[]));
        let Message::Save(call, reply) = receiver.recv().unwrap() else {
            panic!("game.saves sends a list request");
        };
        assert_eq!(call, SaveCall::List);
        reply
            .send(SaveReply::Listed(Ok(vec![Entry {
                filename: "ireland.cksave".to_owned(),
                name: "Ireland".to_owned(),
                written: Some("2026-11-13T12:34:56Z".to_owned()),
                date: Some("3 Mar 771".to_owned()),
                this_build: Some(false),
                fit: Fit::Version,
            }])))
            .unwrap();
        let returned = caller.join().unwrap().unwrap();
        let [ConfigValue::Table(result)] = returned.as_slice() else {
            panic!("the host returns one completed result table");
        };
        assert_eq!(result.get("ok"), Some(&ConfigValue::Bool(true)));
        let Some(ConfigValue::Array(entries)) = result.get("entries") else {
            panic!("the list result carries the save entries");
        };
        let [ConfigValue::Table(entry)] = entries.as_slice() else {
            panic!("one save is listed");
        };
        assert_eq!(
            entry.get("fit"),
            Some(&ConfigValue::String("version".to_owned()))
        );
        assert_eq!(
            entry.get("this_build"),
            Some(&ConfigValue::String("no".to_owned()))
        );

        let (sender, receiver) = std::sync::mpsc::channel();
        let save = save_function(SimControls::new(sender), SaveCall::New);
        let caller =
            std::thread::spawn(move || save(&[Some(ConfigValue::String("Campaign".to_owned()))]));
        let Message::Save(call, reply) = receiver.recv().unwrap() else {
            panic!("game.save sends its requested name");
        };
        assert_eq!(call, SaveCall::New("Campaign".to_owned()));
        reply
            .send(SaveReply::Finished(Err("name is invalid".to_owned())))
            .unwrap();
        let returned = caller.join().unwrap().unwrap();
        let [ConfigValue::Table(result)] = returned.as_slice() else {
            panic!("the save result is a table");
        };
        assert_eq!(result.get("ok"), Some(&ConfigValue::Bool(false)));
        assert_eq!(
            result.get("reason"),
            Some(&ConfigValue::String("name is invalid".to_owned()))
        );
    }

    #[test]
    fn dirty_mark_and_guarded_load_use_the_ordered_route_and_guarded_result_is_async() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let controls = SimControls::new(sender);
        let mark = mark_unsaved_function(controls.clone());
        assert!(mark(&[]).unwrap().is_empty());
        assert_eq!(receiver.recv().unwrap(), Message::MarkUnsaved);

        let guarded = guarded_load_function(controls.clone());
        assert!(
            guarded(&[Some(ConfigValue::String("slot.cksave".to_owned()))])
                .unwrap()
                .is_empty()
        );
        let Message::GuardedLoad { filename, reply } = receiver.recv().unwrap() else {
            panic!("guarded load is sent on the simulation route");
        };
        assert_eq!(filename, "slot.cksave");
        reply.send(GuardedLoadResult {
            filename: filename.clone(),
            result: Err("unsaved".to_owned()),
        });
        let result = controls.take_guarded_result().unwrap();
        assert_eq!(result.filename, "slot.cksave");
        assert_eq!(
            guarded_result_arguments(&result),
            vec![
                ConfigValue::String("slot.cksave".to_owned()),
                error_value("unsaved".to_owned()),
            ]
        );
    }

    #[test]
    fn find_entries_uses_the_requested_world_revision_and_composed_names() {
        let cache = Arc::new(ViewCache::default());
        let mut view = EconomyView::default();
        view.characters.cards.push(CharacterCard {
            id: 7,
            name: "Máel".to_owned(),
            epithet: "Frossach".to_owned(),
            place: "ailech".to_owned(),
            ..CharacterCard::default()
        });
        cache.post(1, Arc::new(view));
        cache.note_world(1, 0);
        let places = vec![Place {
            id: "ailech".into(),
            name: "Grianán Ailigh".into(),
            name_en: None,
            kind: "town".into(),
            size: 1,
            abandoned: false,
        }];
        let find = find_entries_function(
            Arc::clone(&cache),
            places,
            vec![("erne".into(), "Erne".into())],
            PlaceNameSetting::default(),
        );
        let entries = find(&[Some(ConfigValue::Integer(1))]).unwrap();
        let ConfigValue::Array(rows) = &entries[0] else {
            panic!("entries array")
        };
        assert_eq!(rows.len(), 3);
        let ConfigValue::Table(character) = &rows[1] else {
            panic!("character row")
        };
        assert_eq!(
            character.get("name"),
            Some(&ConfigValue::String("Máel Frossach".into()))
        );
        assert_eq!(
            character.get("place"),
            Some(&ConfigValue::String("Grianán Ailigh".into()))
        );
        cache.post(2, view_with(1));
        cache.note_world(2, 1);
        assert!(find(&[Some(ConfigValue::Integer(1))]).is_err());
    }

    /// A place with its local name and, perhaps, an English one.
    fn named_place(id: &str, local: &str, english: Option<&str>) -> Place {
        Place {
            id: id.into(),
            name: local.into(),
            name_en: english.map(Into::into),
            kind: "monastery".into(),
            size: 1,
            abandoned: false,
        }
    }

    fn field<'a>(row: &'a ConfigValue, key: &str) -> Option<&'a str> {
        let ConfigValue::Table(fields) = row else {
            panic!("a table")
        };
        match fields.get(key) {
            Some(ConfigValue::String(value)) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn game_places_gives_the_shown_name_by_the_choice_each_time_it_is_called() {
        let places = vec![
            named_place("birr", "Biorra", Some("Birr")),
            named_place("emain_macha", "Emain Macha", None),
        ];
        let names = PlaceNameSetting::default();
        let function = places_function(places, names.clone());
        let rows = |function: &HostFunction| {
            let ConfigValue::Array(rows) = function(&[]).unwrap().remove(0) else {
                panic!("an array")
            };
            rows
        };
        // English, the default: the English name, and the local one where there is none.
        let english = rows(&function);
        assert_eq!(field(&english[0], "name"), Some("Birr"));
        assert_eq!(field(&english[0], "local_name"), Some("Biorra"));
        assert_eq!(field(&english[0], "english_name"), Some("Birr"));
        assert_eq!(field(&english[1], "name"), Some("Emain Macha"));
        assert_eq!(field(&english[1], "english_name"), None);
        // The same function, called again after the choice changed: no restart, no rebuilt function.
        names.set(crate::place_names::LOCAL);
        let local = rows(&function);
        assert_eq!(field(&local[0], "name"), Some("Biorra"));
        assert_eq!(field(&local[1], "name"), Some("Emain Macha"));
        assert_eq!(field(&local[0], "id"), Some("birr"));
    }

    #[test]
    fn find_entries_carry_the_shown_name_and_the_other_one_under_either_choice() {
        let cache = Arc::new(ViewCache::default());
        let mut view = EconomyView::default();
        view.characters.cards.push(CharacterCard {
            id: 7,
            name: "Máel".to_owned(),
            place: "birr".to_owned(),
            ..CharacterCard::default()
        });
        cache.post(1, Arc::new(view));
        cache.note_world(1, 0);
        let names = PlaceNameSetting::default();
        let find = find_entries_function(
            Arc::clone(&cache),
            vec![
                named_place("birr", "Biorra", Some("Birr")),
                named_place("emain_macha", "Emain Macha", None),
            ],
            Vec::new(),
            names.clone(),
        );
        let entries = |find: &HostFunction| {
            let ConfigValue::Array(rows) =
                find(&[Some(ConfigValue::Integer(1))]).unwrap().remove(0)
            else {
                panic!("an array")
            };
            rows
        };
        let english = entries(&find);
        assert_eq!(field(&english[0], "name"), Some("Birr"));
        assert_eq!(field(&english[0], "other"), Some("Biorra"));
        assert_eq!(field(&english[1], "name"), Some("Emain Macha"));
        assert_eq!(field(&english[1], "other"), None, "one name has no other");
        assert_eq!(
            field(&english[2], "place"),
            Some("Birr"),
            "a character's place is the shown name"
        );
        names.set(crate::place_names::LOCAL);
        let local = entries(&find);
        assert_eq!(field(&local[0], "name"), Some("Biorra"));
        assert_eq!(field(&local[0], "other"), Some("Birr"));
        assert_eq!(field(&local[2], "place"), Some("Biorra"));
    }

    #[test]
    fn blights_are_bound_to_each_event_revision_and_place() {
        use crate::game::events::{Action, Event, EventKind};
        let cache = Arc::new(ViewCache::default());
        let make = |hour, action| Event {
            at: ck_sim::GameTime::from_date(ck_sim::Date {
                year: 768,
                month: 10,
                day: 12,
                hour,
            })
            .unwrap(),
            action,
            kind: EventKind::Blight {
                good: "oats".into(),
                place: "clonmacnoise".into(),
                place_order: 0,
            },
        };
        cache.post_events(1, Arc::new(vec![]));
        cache.post_events(2, Arc::new(vec![make(9, Action::Begins)]));
        cache.post_events(
            3,
            Arc::new(vec![make(9, Action::Begins), make(20, Action::Ends)]),
        );
        let blights = blights_function(cache);
        let call = |place: &str, revision| {
            blights(&[
                Some(ConfigValue::String(place.into())),
                Some(ConfigValue::Integer(revision)),
            ])
            .unwrap()
            .remove(0)
        };
        assert_eq!(call("clonmacnoise", 1), ConfigValue::Array(vec![]));
        assert_eq!(
            call("clonmacnoise", 2),
            ConfigValue::Array(vec![ConfigValue::String("oats".into())])
        );
        assert_eq!(call("elsewhere", 2), ConfigValue::Array(vec![]));
        assert_eq!(call("clonmacnoise", 3), ConfigValue::Array(vec![]));
        assert_eq!(
            call("clonmacnoise", 2),
            ConfigValue::Array(vec![ConfigValue::String("oats".into())])
        );
    }

    #[test]
    fn the_event_log_names_a_blight_place_by_the_choice_and_keeps_its_id() {
        use crate::game::events::{Action, Event, EventKind};
        let cache = Arc::new(ViewCache::default());
        cache.post_events(
            1,
            Arc::new(vec![Event {
                at: ck_sim::GameTime::from_date(ck_sim::Date {
                    year: 768,
                    month: 5,
                    day: 2,
                    hour: 6,
                })
                .expect("a date"),
                action: Action::Begins,
                kind: EventKind::Blight {
                    good: "oats".into(),
                    place: "birr".into(),
                    place_order: 0,
                },
            }]),
        );
        let names = PlaceNameSetting::default();
        let events = events_function(
            Arc::clone(&cache),
            vec![named_place("birr", "Biorra", Some("Birr"))],
            names.clone(),
        );
        let row = |events: &HostFunction| {
            let ConfigValue::Array(mut rows) =
                events(&[Some(ConfigValue::Integer(1))]).unwrap().remove(0)
            else {
                panic!("an array")
            };
            rows.remove(0)
        };
        let english = row(&events);
        assert_eq!(field(&english, "place"), Some("Birr"));
        assert_eq!(field(&english, "place_id"), Some("birr"));
        names.set(crate::place_names::LOCAL);
        let local = row(&events);
        assert_eq!(field(&local, "place"), Some("Biorra"));
        assert_eq!(field(&local, "place_id"), Some("birr"));
    }

    #[test]
    fn an_abandoned_places_view_name_and_the_character_tables_follow_the_choice() {
        let cache = Arc::new(ViewCache::default());
        let mut view = EconomyView::default();
        view.places.push(PlaceView {
            id: "birr".to_owned(),
            ..PlaceView::default()
        });
        view.characters.cards.push(CharacterCard {
            id: 7,
            name: "Máel".to_owned(),
            role: "ruler of Biorra".to_owned(),
            role_english: "ruler of Birr".to_owned(),
            place: "birr".to_owned(),
            ..CharacterCard::default()
        });
        cache.post(1, Arc::new(view));
        cache.note_world(1, 0);
        let names = PlaceNameSetting::default();
        let mut abandoned = named_place("birr", "Biorra", Some("Birr"));
        abandoned.abandoned = true;
        let place = place_function(Arc::clone(&cache), vec![abandoned], names.clone());
        let cards = characters_function(Arc::clone(&cache), names.clone());
        let place_name = |place: &HostFunction| {
            field(
                &place(&[
                    Some(ConfigValue::String("birr".to_owned())),
                    Some(ConfigValue::Integer(1)),
                ])
                .unwrap()[0],
                "name",
            )
            .map(str::to_owned)
        };
        let role = |cards: &HostFunction| {
            let ConfigValue::Array(rows) =
                cards(&[Some(ConfigValue::Integer(1))]).unwrap().remove(0)
            else {
                panic!("an array")
            };
            field(&rows[0], "role").map(str::to_owned)
        };
        assert_eq!(place_name(&place).as_deref(), Some("Birr"));
        assert_eq!(role(&cards).as_deref(), Some("ruler of Birr"));
        names.set(crate::place_names::LOCAL);
        assert_eq!(place_name(&place).as_deref(), Some("Biorra"));
        assert_eq!(role(&cards).as_deref(), Some("ruler of Biorra"));
    }

    fn view_with(workers: u64) -> Arc<EconomyView> {
        Arc::new(EconomyView {
            places: vec![PlaceView {
                id: "fixture".to_owned(),
                workers,
                ..PlaceView::default()
            }],
            ..EconomyView::default()
        })
    }

    fn call_place(
        function: &HostFunction,
        id: &str,
        revision: i64,
    ) -> Result<Vec<ConfigValue>, String> {
        function(&[
            Some(ConfigValue::String(id.to_owned())),
            Some(ConfigValue::Integer(revision)),
        ])
    }

    fn workers_at(cache: &ViewCache, revision: u64) -> u64 {
        cache
            .get(revision)
            .unwrap_or_else(|error| panic!("{error}"))
            .place("fixture")
            .unwrap()
            .workers
    }

    fn retired(revision: u64) -> String {
        format!("game.place: revision {revision} is retired")
    }

    #[test]
    fn the_view_cache_keeps_every_pending_calls_view_and_the_last_completed_and_retires_on_completion()
     {
        let cache = ViewCache::default();
        assert_eq!(VIEW_CACHE_CAPACITY, UI_CALL_LIMIT + 1);
        // Three calls are posted; all three views are served, none has completed.
        for revision in 1..=3 {
            cache.post(revision, view_with(revision * 10));
        }
        assert_eq!(cache.held(), 3);
        // The first completes: it is the last completed and stays; the others are pending and stay.
        cache.complete(1);
        assert_eq!(
            [1, 2, 3].map(|revision| workers_at(&cache, revision)),
            [10, 20, 30]
        );
        // The second completes, and the first (older, unpinned) is refused at once, with the cache nowhere near full.
        cache.complete(2);
        assert_eq!(cache.get(1).unwrap_err(), retired(1));
        assert_eq!((workers_at(&cache, 2), workers_at(&cache, 3)), (20, 30));
        assert_eq!(cache.held(), 2);
        // A fourth is posted; the third completes and retires the second.
        cache.post(4, view_with(40));
        cache.complete(3);
        assert_eq!(cache.get(2).unwrap_err(), retired(2));
        assert_eq!((workers_at(&cache, 3), workers_at(&cache, 4)), (30, 40));
        // A revision that was never posted is missing, as is revision 0, which is "no view".
        assert_eq!(
            cache.get(99).unwrap_err(),
            "game.place: revision 99 is missing"
        );
        assert_eq!(
            cache.get(0).unwrap_err(),
            "game.place: revision 0 is missing"
        );
    }

    #[test]
    fn calls_that_carry_one_revision_each_pin_it_and_the_last_completed_view_outlives_nothing_but_a_later_completion()
     {
        let cache = ViewCache::default();
        // Two calls carry revision 5 (a snapshot an hour reuses the day's view); a third carries 6.
        cache.post(5, view_with(50));
        cache.post(5, view_with(999));
        cache.post(6, view_with(60));
        assert_eq!(workers_at(&cache, 5), 50, "a revision names its first view");
        assert_eq!(cache.held(), 2);
        cache.complete(5);
        assert_eq!(workers_at(&cache, 5), 50, "the other call still pins it");
        cache.complete(5);
        assert_eq!(
            workers_at(&cache, 5),
            50,
            "no pin left, but it is the last completed"
        );
        cache.complete(6);
        assert_eq!(cache.get(5).unwrap_err(), retired(5));
        assert_eq!(workers_at(&cache, 6), 60);
    }

    #[test]
    fn a_post_that_failed_leaves_no_pin_and_the_cache_never_exceeds_its_bound() {
        let cache = ViewCache::default();
        cache.post(1, view_with(10));
        cache.complete(1);
        // A post that fails: the view it was announced with leaves, and the last completed one stays.
        cache.post(2, view_with(20));
        cache.unpost(2);
        assert_eq!(cache.get(2).unwrap_err(), retired(2));
        assert_eq!(workers_at(&cache, 1), 10);
        // The worst case: the call limit's worth of pending calls (one place is kept for lifecycle calls) and the last completed.
        for revision in 3..3 + u64::try_from(UI_CALL_LIMIT - 1).unwrap() {
            cache.post(revision, view_with(revision));
        }
        assert_eq!(cache.held(), UI_CALL_LIMIT);
        assert!(cache.held() <= VIEW_CACHE_CAPACITY);
    }

    #[test]
    fn game_place_returns_the_revisions_view_and_refuses_what_it_cannot_serve() {
        let cache = Arc::new(ViewCache::default());
        cache.post(7, view_with(120));
        let function = place_function(Arc::clone(&cache), vec![], PlaceNameSetting::default());
        let Ok(returned) = call_place(&function, "fixture", 7) else {
            panic!("a held revision of a known place is served");
        };
        let [ConfigValue::Table(fields)] = returned.as_slice() else {
            panic!("one table");
        };
        assert_eq!(fields["id"], ConfigValue::String("fixture".to_owned()));
        assert_eq!(fields["workers"], ConfigValue::Integer(120));
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "basic_housing_capacity",
                "basic_housing_occupants",
                "buildings",
                "crowding_pm",
                "dependents",
                "development_pm",
                "farmland_quality_pm",
                "field_room",
                "food_days",
                "footprint_m2",
                "id",
                "land",
                "last_month",
                "makeshift_housing_capacity",
                "makeshift_housing_occupants",
                "max_buildable_m2",
                "overflow_lost",
                "pops",
                "prepared_m2",
                "prices",
                "storage_capacity_l",
                "storage_used_l",
                "workers"
            ]
        );
        assert_eq!(
            call_place(&function, "elsewhere", 7).unwrap_err(),
            "game.place: no place `elsewhere` at revision 7"
        );
        assert_eq!(
            call_place(&function, "fixture", 8).unwrap_err(),
            "game.place: revision 8 is missing"
        );
        assert_eq!(
            call_place(&function, "fixture", -1).unwrap_err(),
            "game.place: revision -1 is missing"
        );
        // A later call completes: revision 7 is retired, and the refusal names it.
        cache.post(8, view_with(1));
        cache.complete(7);
        cache.complete(8);
        assert_eq!(
            call_place(&function, "fixture", 7).unwrap_err(),
            "game.place: revision 7 is retired"
        );
        assert_eq!(
            function(&[Some(ConfigValue::String("fixture".to_owned()))]).unwrap_err(),
            "game.place: expected a place id and a revision"
        );
        assert_eq!(
            function(&[None, None]).unwrap_err(),
            "game.place: expected a place id and a revision"
        );
    }

    #[test]
    fn game_place_exposes_order_values_and_omits_them_before_the_price_step() {
        let cache = Arc::new(ViewCache::default());
        let make_price =
            |good: &str, tags: &[&str], bought, offered| crate::game::view::PriceView {
                good: good.to_owned(),
                tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
                price_milli: 96_500,
                base: 100,
                bought,
                offered,
            };
        cache.post(
            1,
            Arc::new(EconomyView {
                places: vec![PlaceView {
                    id: "fixture".to_owned(),
                    prices: vec![
                        make_price("oats", &["food", "grain"], Some(1240), Some(1410)),
                        make_price("barley", &[], None, None),
                    ],
                    ..PlaceView::default()
                }],
                ..EconomyView::default()
            }),
        );
        let returned = call_place(
            &place_function(cache, vec![], PlaceNameSetting::default()),
            "fixture",
            1,
        )
        .unwrap();
        let [ConfigValue::Table(place)] = returned.as_slice() else {
            panic!("one place table");
        };
        let ConfigValue::Array(prices) = &place["prices"] else {
            panic!("price array");
        };
        let ConfigValue::Table(oats) = &prices[0] else {
            panic!("oats price table");
        };
        assert_eq!(oats["bought"], ConfigValue::Integer(1240));
        assert_eq!(oats["offered"], ConfigValue::Integer(1410));
        assert_eq!(
            oats["tags"],
            ConfigValue::Array(vec![text("food"), text("grain")])
        );
        let ConfigValue::Table(barley) = &prices[1] else {
            panic!("barley price table");
        };
        assert!(!barley.contains_key("bought"));
        assert!(!barley.contains_key("offered"));
        assert_eq!(barley["tags"], ConfigValue::Array(Vec::new()));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The table contract test checks the full exposed place and pop projection in one fixture."
    )]
    fn a_places_table_carries_every_tab_field_with_litres_for_volumes() {
        let place = PlaceView {
            id: "fixture".to_owned(),
            temperature: None,
            wind: None,
            workers: 80,
            dependents: 40,
            food_days: 215,
            storage_used_ml: 5_400_999,
            storage_capacity_ml: 86_400_000,
            prices: vec![
                crate::game::view::PriceView {
                    good: "oats".to_owned(),
                    tags: vec!["food".to_owned()],
                    price_milli: 100_500,
                    base: 100,
                    bought: Some(u128::from(u64::MAX) + 5),
                    offered: Some(1410),
                },
                crate::game::view::PriceView {
                    good: "barley".to_owned(),
                    tags: Vec::new(),
                    price_milli: 2000,
                    base: 2,
                    bought: Some(1240),
                    offered: Some(0),
                },
                crate::game::view::PriceView {
                    good: "ale".to_owned(),
                    tags: vec!["food".to_owned()],
                    price_milli: 2500,
                    base: 2,
                    bought: None,
                    offered: None,
                },
            ],
            land: crate::game::view::LandView {
                usable: BTreeMap::from([("pasture".to_owned(), 7)]),
                land: BTreeMap::from([("pasture".to_owned(), 9)]),
                sea: 2,
            },
            last_month: crate::game::view::LastMonthView {
                closed: true,
                produced: vec![
                    crate::game::view::GoodCounterView {
                        good: "oats".to_owned(),
                        units: 12,
                        value_milli: Some(1_158_000),
                    },
                    crate::game::view::GoodCounterView {
                        good: "barley".to_owned(),
                        units: 1,
                        value_milli: Some(u128::MAX),
                    },
                ],
                used_by_pops: vec![crate::game::view::GoodCounterView {
                    good: "oats".to_owned(),
                    units: 2,
                    value_milli: Some(193_000),
                }],
                lost: vec![crate::game::view::GoodCounterView {
                    good: "ale".to_owned(),
                    units: 3,
                    value_milli: None,
                }],
            },
            overflow_lost: 0,
            projected_yield: 0,
            has_economy: true,
            footprint_m2: 4_200,
            prepared_m2: 24_200,
            max_buildable_m2: 25_000,
            development_pm: 1000,
            farmland_quality_pm: 550,
            field_room: 12,
            basic_housing_capacity: 40,
            basic_housing_occupants: 38,
            makeshift_housing_capacity: 10,
            makeshift_housing_occupants: 2,
            crowding_pm: 2000,
            pops: vec![PopView {
                jobs: Vec::new(),
                stock: Vec::new(),
                room_ml: 0,
                kind: "peasant".to_owned(),
                culture: "ulaid".to_owned(),
                religion: "christianity".to_owned(),
                workers: 80,
                dependents: 40,
                employed: 70,
                unemployed: 10,
                food_days: 215,
                food: vec![crate::game::view::FoodView {
                    good: "oats".to_owned(),
                    units: 300,
                    unit: "bushel".to_owned(),
                    share_pm: 658,
                }],
                necessities: 0,
                niceties: 0,
                luxuries: 0,
                satisfaction: -5,
                equilibrium: -3,
                births: 2,
                deaths: 1,
                basic_housing: 38,
                makeshift_housing: 2,
                crowding_pm: Some(2000),
            }],
            buildings: vec![BuildingView {
                jobs: Vec::new(),
                abandoned: false,
                stock: Vec::new(),
                made: Vec::new(),
                kind: "farm".to_owned(),
                levels: 3,
                has_methods: true,
                units: 15,
                free_units: 0,
                methods: vec![("mixed_farming".to_owned(), 3)],
                filled: 25,
                open: 5,
                herds: vec![crate::game::view::HerdView {
                    species: "cattle".to_owned(),
                    head: 27,
                    maximum: 36,
                }],
                store_used_ml: 1_500,
                store_capacity_ml: 3_500_000,
                wage: 0,
                wages: vec![crate::game::view::WageView {
                    month: 5,
                    year: None,
                    wage_milli: 1_234,
                    paid: None,
                }],
                housing_occupants: 38,
            }],
        };
        let ConfigValue::Table(fields) = place_table(&place) else {
            panic!("a table");
        };
        assert_eq!(fields["storage_used_l"], ConfigValue::Integer(5_400));
        assert_eq!(fields["storage_capacity_l"], ConfigValue::Integer(86_400));
        assert_eq!(fields["footprint_m2"], ConfigValue::Integer(4_200));
        assert_eq!(fields["prepared_m2"], ConfigValue::Integer(24_200));
        assert_eq!(fields["max_buildable_m2"], ConfigValue::Integer(25_000));
        assert_eq!(fields["development_pm"], ConfigValue::Integer(1000));
        assert_eq!(fields["farmland_quality_pm"], ConfigValue::Integer(550));
        assert_eq!(fields["field_room"], ConfigValue::Integer(12));
        let ConfigValue::Table(land) = &fields["land"] else {
            panic!("a land table");
        };
        assert_eq!(land["sea"], ConfigValue::Integer(2));
        let ConfigValue::Table(usable) = &land["usable"] else {
            panic!("a terrain table");
        };
        assert_eq!(usable["pasture"], ConfigValue::Integer(7));
        let ConfigValue::Table(last_month) = &fields["last_month"] else {
            panic!("a last month table");
        };
        assert_eq!(last_month["closed"], ConfigValue::Bool(true));
        assert!(!last_month.contains_key("used_as_inputs"));
        let ConfigValue::Array(produced) = &last_month["produced"] else {
            panic!("an array");
        };
        let ConfigValue::Table(oats) = &produced[0] else {
            panic!("a good counter table");
        };
        assert_eq!(oats["units"], ConfigValue::Integer(12));
        assert_eq!(oats["value_milli"], ConfigValue::Integer(1_158_000));
        let ConfigValue::Table(barley) = &produced[1] else {
            panic!("a good counter table");
        };
        assert_eq!(barley["value_milli"], ConfigValue::Integer(i64::MAX));
        let ConfigValue::Array(lost) = &last_month["lost"] else {
            panic!("an array");
        };
        let ConfigValue::Table(ale) = &lost[0] else {
            panic!("a good counter table");
        };
        assert!(!ale.contains_key("value_milli"));
        let ConfigValue::Array(price_rows) = &fields["prices"] else {
            panic!("an array");
        };
        let ConfigValue::Table(price) = &price_rows[0] else {
            panic!("a table");
        };
        assert_eq!(price["tags"], ConfigValue::Array(vec![text("food")]));
        assert_eq!(price["price_milli"], ConfigValue::Integer(100_500));
        assert_eq!(price["bought"], ConfigValue::Integer(i64::MAX));
        assert_eq!(price["offered"], ConfigValue::Integer(1410));
        let ConfigValue::Table(second_price) = &price_rows[1] else {
            panic!("a price table");
        };
        assert_eq!(second_price["tags"], ConfigValue::Array(Vec::new()));
        assert_eq!(second_price["bought"], ConfigValue::Integer(1240));
        let ConfigValue::Table(unavailable) = &price_rows[2] else {
            panic!("a price table");
        };
        assert_eq!(unavailable["tags"], ConfigValue::Array(vec![text("food")]));
        assert!(!unavailable.contains_key("bought"));
        assert!(!unavailable.contains_key("offered"));
        let ConfigValue::Array(pops) = &fields["pops"] else {
            panic!("an array");
        };
        let ConfigValue::Table(pop) = &pops[0] else {
            panic!("a table");
        };
        assert_eq!(pop["satisfaction"], ConfigValue::Integer(-5));
        assert_eq!(pop["equilibrium"], ConfigValue::Integer(-3));
        assert_eq!(pop["employed"], ConfigValue::Integer(70));
        let ConfigValue::Array(food) = &pop["food"] else {
            panic!("a food array");
        };
        let ConfigValue::Table(oats) = &food[0] else {
            panic!("a food row");
        };
        assert_eq!(oats["share_pm"], ConfigValue::Integer(658));
        let ConfigValue::Array(buildings) = &fields["buildings"] else {
            panic!("an array");
        };
        let ConfigValue::Table(farm) = &buildings[0] else {
            panic!("a table");
        };
        assert_eq!(farm["store_used_l"], ConfigValue::Integer(1));
        let ConfigValue::Array(wages) = &farm["wages"] else {
            panic!("a wage array");
        };
        let ConfigValue::Table(may) = &wages[0] else {
            panic!("a wage row");
        };
        assert_eq!(may["wage_milli"], ConfigValue::Integer(1_234));
        assert!(!may.contains_key("year"));
        assert!(!may.contains_key("paid"));
        assert_eq!(farm["store_capacity_l"], ConfigValue::Integer(3_500));
        let ConfigValue::Array(herds) = &farm["herds"] else {
            panic!("an array");
        };
        let ConfigValue::Table(cattle) = &herds[0] else {
            panic!("a table");
        };
        assert_eq!(cattle["maximum"], ConfigValue::Integer(36));
    }

    /// `game.places()` over the shipped Ireland 768 scenario: its 123 locations in file order, the first and last literal, each with the
    /// kind's id and the authored size that `game_select` reports for the same location.
    #[test]
    fn places_returns_ireland_768s_locations_in_file_order_with_the_select_fields() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the shipped scenario loads");
        let places = places_of(&scenario);
        assert_eq!(places.len(), 125);
        assert_eq!(
            places[0],
            Place {
                id: "armagh".to_owned(),
                name: "Ard Mhacha".to_owned(),
                name_en: Some("Armagh".to_owned()),
                kind: "monastery".to_owned(),
                size: 4,
                abandoned: false,
            }
        );
        assert_eq!(
            places[122],
            Place {
                id: "monaincha".to_owned(),
                name: "Loch Cré".to_owned(),
                name_en: Some("Monaincha".to_owned()),
                kind: "monastery".to_owned(),
                size: 1,
                abandoned: false,
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
                abandoned: place.abandoned,
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
        // The host function returns them as one array of `{id, name, local_name, english_name, kind, size}` and takes no argument.
        let function = places_function(places.clone(), PlaceNameSetting::default());
        let Ok(returned) = function(&[]) else {
            panic!("game.places() with no arguments succeeds");
        };
        let [ConfigValue::Array(rows)] = returned.as_slice() else {
            panic!("one array");
        };
        assert_eq!(rows.len(), 125);
        let ConfigValue::Table(first) = &rows[0] else {
            panic!("a table per place");
        };
        assert_eq!(
            first.keys().map(String::as_str).collect::<Vec<_>>(),
            ["english_name", "id", "kind", "local_name", "name", "size"]
        );
        // The default choice shows the English name; the local one is always there too.
        assert_eq!(first["name"], ConfigValue::String("Armagh".to_owned()));
        assert_eq!(
            first["english_name"],
            ConfigValue::String("Armagh".to_owned())
        );
        assert_eq!(
            first["local_name"],
            ConfigValue::String("Ard Mhacha".to_owned())
        );
        let refused = function(&[Some(ConfigValue::Integer(1))]).unwrap_err();
        assert_eq!(refused, "game.places: takes no arguments");
    }

    #[test]
    fn scenario_returns_the_literal_id_and_rejects_arguments() {
        let function = scenario_function("ireland_768".to_owned());
        assert_eq!(
            function(&[]),
            Ok(vec![ConfigValue::String("ireland_768".to_owned())])
        );
        assert_eq!(
            function(&[Some(ConfigValue::String("other".to_owned()))]),
            Err("game.scenario: takes no arguments".to_owned())
        );
        assert_eq!(
            function(&[None]),
            Err("game.scenario: takes no arguments".to_owned())
        );
    }
}
