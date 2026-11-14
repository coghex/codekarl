//! The `game` extension of the UI VM: how the simulation reaches the UI.
//!
//! `game.on_time(fn | nil)` registers the callback `game_time` (and exists only when the configuration has `[game] scenario`).
//! The UI link posts it with the latest [`SimSnapshot`] as a table, `{year, month, day, hour, speed, paused, economy_revision}`,
//! where `paused` is `false` or the reason as the string `"player"`, `"error"` or `"load"`: see `ui_link` for when.
//!
//! `game.place(id, revision)` returns the economy view of a place at a revision a `game_time` carried (milestone 9, design §6). It
//! reads only the [`ViewCache`] the UI link fills when it posts a snapshot, never the live simulation.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ck_lua::{ConfigValue, Extension, HostFunction};

use crate::game::controls::{self, SimControls};
use crate::game::slot::{PauseReason, SimSnapshot};
use crate::game::view::{BuildingView, EconomyView, PlaceView, PopView};
use crate::vm::UI_CALL_LIMIT;

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
        .function("control", control_function(controls.clone()))
        .function("pause", pause_function(controls.clone()))
        .function("set_modal", set_modal_function(controls))
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

/// `game.set_map_mode(name)`: shows the map mode `"stockpile"`, `"yield"` or `"elevation"`, or the plain map for `"off"`. The values of the first two come
/// from the newest view the UI has been given; the elevation mode needs none. An unknown name, or a map the lane refuses a table for, is an error.
pub(crate) fn set_map_mode_function(modes: Arc<crate::map_modes::MapModes>) -> HostFunction {
    Arc::new(move |arguments| {
        let [Some(ConfigValue::String(name))] = arguments else {
            return Err("game.set_map_mode: expected one string argument (name)".to_owned());
        };
        let Some(mode) = crate::map_modes::Mode::parse(name) else {
            return Err(format!(
                "game.set_map_mode: unknown mode `{name}` (expected off, stockpile, yield or elevation)"
            ));
        };
        if let Err(message) = modes.set(mode) {
            let _ = ck_log::try_log!(ck_log::Level::Warn, "map", "{message}");
            return Err(format!("game.set_map_mode: {message}"));
        }
        Ok(Vec::new())
    })
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
    /// Told of each view as it is posted, once (the map modes' values come from the newest view).
    observer: std::sync::OnceLock<ViewObserver>,
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
            self.retired_through = self.retired_through.max(revision);
        }
    }
}

impl ViewCache {
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
        let inner = self.locked();
        if let Some(view) = inner.views.get(&revision) {
            return Ok(Arc::clone(view));
        }
        // Revision 0 is "no view" and was never held, whatever has left since.
        if revision != 0 && revision <= inner.retired_through {
            Err(format!("game.place: revision {revision} is retired"))
        } else {
            Err(format!("game.place: revision {revision} is missing"))
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

fn pop_table(pop: &PopView) -> ConfigValue {
    table([
        ("kind", text(&pop.kind)),
        ("culture", text(&pop.culture)),
        ("religion", text(&pop.religion)),
        ("workers", integer(pop.workers)),
        ("dependents", integer(pop.dependents)),
        ("employed", integer(pop.employed)),
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
        ("kind", text(&building.kind)),
        ("levels", integer(building.levels)),
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
    table([
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
    ])
}

/// `game.place(id, revision)`: the economy view of the place `id` at `revision`, from the cache and never from live state. It
/// refuses, by name, a revision that is missing or retired, a place the view does not hold and arguments of the wrong kind.
pub(crate) fn place_function(cache: Arc<ViewCache>) -> HostFunction {
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
        Ok(vec![place_table(place)])
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
        (
            "economy_revision".to_owned(),
            integer(snapshot.economy_revision),
        ),
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

    fn view_with(workers: u64) -> Arc<EconomyView> {
        Arc::new(EconomyView {
            places: vec![PlaceView {
                id: "fixture".to_owned(),
                workers,
                ..PlaceView::default()
            }],
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
        let function = place_function(Arc::clone(&cache));
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
            }),
        );
        let returned = call_place(&place_function(cache), "fixture", 1).unwrap();
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
                births: 2,
                deaths: 1,
                basic_housing: 38,
                makeshift_housing: 2,
                crowding_pm: Some(2000),
            }],
            buildings: vec![BuildingView {
                kind: "farm".to_owned(),
                levels: 3,
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
