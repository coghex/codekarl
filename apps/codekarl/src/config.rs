//! The configuration file: TOML, read only from `--config`.
//!
//! ```toml
//! [log]
//! level = "info"          # error | warn | info | debug | trace   (default info)
//! timestamps = true       # default true
//! colour = "auto"         # auto | always | never                 (default auto)
//!
//! [log.categories]        # optional per-category levels
//! runtime = "debug"
//!
//! [runtime]               # optional owner deadlines, each in seconds, greater than 0 and at most 60
//! start_timeout = 5       # default 5
//! stop_timeout = 5        # default 5
//! cleanup_timeout = 1     # default 1
//!
//! [window]                # optional; without a `backend` (or without the table) the run is headless
//! backend = "native-metal"   # native-metal | glfw-vulkan; `--headless` and `--backend` override it

//! [map]                   # optional; adds the map camera and six map input actions
//! directory = "../maps/fixture"
//! pan_pixels_per_frame = 4 # 1..=32, default 4
//! zoom_steps_per_second = 20 # 1..=60, default 20
//! scroll_points_per_step = 24 # 1..=256, default 24
//!
//! [game]                  # optional; without it there is no simulation. Adds the time and save input actions
//! scenario = "../scenarios/ireland-768"   # a scenario directory: its map replaces `[map] directory` (both is an error)
//! saves = "saves"         # the quicksave directory, default `saves` beside this file
//!
//! [input.bindings]        # optional; action name = the physical-key bindings that hold it (design §7.1)
//! close = ["Escape"]      # the default; an explicit empty list unbinds an action
//! move_left = ["KeyA", "ArrowLeft"]
//! reload_texture = ["Control+KeyR"]
//!
//! [frame]                 # optional; the paced UI update of the native-metal loop (design §7.1)
//! updates_per_second = 60 # an integer, 1 to 240 (default 60)
//! max_delta_seconds = 0.1 # a number from 0.000000001 (one nanosecond, the timer resolution) to 1.0 (default 0.1)
//!
//! [ui.tooltips]          # optional tooltip behavior, read at startup
//! delay = 0.3            # 0..=2 seconds in 0.1-second steps
//! lock = "timer"         # timer | middle_mouse | disabled
//! lock_timer = 1.0       # 0.3..=3 seconds in 0.1-second steps
//!
//! [lua.ui]                # one table per Lua VM; the name is lowercase letters, digits and `_`
//! entry = "../scripts/ui/main.lua"   # relative to the directory of this file
//! ```
//!
//! The built-in defaults equal an empty file, which runs no VMs and no window. Every table and key is
//! optional (a `lua.<name>` table needs its `entry`), and an unknown key at any level is an error, not
//! ignored. `[input]` has one sub-table, `bindings`; what an entry may say is `ck-input`'s grammar, and its refusal
//! fails the load with its message naming the action and the entry. Both backends' loops use the bindings.
//! `[map].scroll_points_per_step` applies to precise Cocoa point deltas. GLFW exposes fractional line offsets on
//! both platforms; on macOS it scales precise trackpad deltas by 0.1 and drops the precision flag, so this setting
//! does not apply to GLFW.
//! `[frame]` has two keys, each optional with its default; a value outside its range fails the load naming the key, the
//! value and the range. Both backends' loops pace the `frame_update` callback by them (and post nothing until a script
//! registers one). Whether a configured backend can be opened is not checked here:
//! only a windowed run checks it (see [`crate::mode`]).
//! The whole file, not only the `lua` tables, is also copied to every VM as its `config`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ck_log::{ColourMode, Filter, Level, Settings};
use ck_lua::ConfigValue;
use ck_render_2d::Filter as ImageFilter;
use ck_render_2d::SceneLimits;
use ck_resources::ResourceLimits;
use ck_window::{Bindings, SettingsError, UpdateSettings};
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::mode::Backend;

/// A log level named in the file.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(try_from = "String")]
struct LevelName(Level);

impl TryFrom<String> for LevelName {
    type Error = ck_log::ParseLevelError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse().map(Self)
    }
}

/// A colour mode named in the file.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(try_from = "String")]
struct ColourName(ColourMode);

/// The text is not a colour mode.
#[derive(Debug)]
struct UnknownColour(String);

impl fmt::Display for UnknownColour {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown colour {:?}: expected auto, always or never",
            self.0
        )
    }
}

impl TryFrom<String> for ColourName {
    type Error = UnknownColour;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        match text.as_str() {
            "auto" => Ok(Self(ColourMode::Auto)),
            "always" => Ok(Self(ColourMode::Always)),
            "never" => Ok(Self(ColourMode::Never)),
            _ => Err(UnknownColour(text)),
        }
    }
}

/// A category named as a key of `[log.categories]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
struct CategoryName(String);

impl TryFrom<String> for CategoryName {
    type Error = ck_log::InvalidCategory;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        // `Filter` decides what a valid category is, and names the offending text in its error.
        Filter::new(Level::Info).set_category(&text, Level::Info)?;
        Ok(Self(text))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    input: InputTable,
    #[serde(default)]
    log: Log,
    #[serde(default)]
    runtime: Runtime,
    #[serde(default)]
    resources: ResourceTable,
    #[serde(default)]
    window: Window,
    #[serde(default)]
    map: Option<MapTable>,
    #[serde(default)]
    game: Option<GameTable>,
    #[serde(default)]
    render: RenderTable,
    #[serde(default)]
    ui: UiTable,
    #[serde(default)]
    frame: FrameTable,
    #[serde(default)]
    lua: BTreeMap<VmName, Vm>,
}

/// `[map]`: the map directory and the paced camera control rates.
#[derive(Debug, Default)]
struct MapFields {
    directory: Option<PathBuf>,
    pan_pixels_per_frame: Option<i64>,
    zoom_steps_per_second: Option<i64>,
    scroll_points_per_step: Option<i64>,
}

impl<'de> Deserialize<'de> for MapFields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(deny_unknown_fields, default)]
        struct Fields {
            directory: Option<PathBuf>,
            pan_pixels_per_frame: Option<i64>,
            zoom_steps_per_second: Option<i64>,
            scroll_points_per_step: Option<i64>,
        }

        let fields: Fields = table_only(deserializer, "a table for `map`")?;
        Ok(Self {
            directory: fields.directory,
            pan_pixels_per_frame: fields.pan_pixels_per_frame,
            zoom_steps_per_second: fields.zoom_steps_per_second,
            scroll_points_per_step: fields.scroll_points_per_step,
        })
    }
}

#[derive(Debug, Default)]
struct MapTable(MapFields);

impl<'de> Deserialize<'de> for MapTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `map`").map(Self)
    }
}

/// `[game]`: the scenario the simulation runs and the quicksave directory.
#[derive(Debug, Default)]
struct GameFields {
    scenario: Option<PathBuf>,
    saves: Option<PathBuf>,
}

impl<'de> Deserialize<'de> for GameFields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(deny_unknown_fields, default)]
        struct Fields {
            scenario: Option<PathBuf>,
            saves: Option<PathBuf>,
        }

        let fields: Fields = table_only(deserializer, "a table for `game`")?;
        Ok(Self {
            scenario: fields.scenario,
            saves: fields.saves,
        })
    }
}

#[derive(Debug, Default)]
struct GameTable(GameFields);

impl<'de> Deserialize<'de> for GameTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `game`").map(Self)
    }
}

/// Deserializes a `T` from a table (`[name]` or an inline table) and from nothing else. Serde's derived
/// struct deserialization would also accept an array, reading its members positionally.
fn table_only<'de, T, D>(deserializer: D, expecting: &'static str) -> Result<T, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    struct TableOnly<T>(&'static str, PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for TableOnly<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(TableOnly(expecting, PhantomData))
}

/// The keys of the `log` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Fields {
    level: LevelName,
    timestamps: bool,
    colour: ColourName,
    categories: BTreeMap<CategoryName, LevelName>,
}

impl Default for Fields {
    fn default() -> Self {
        Self {
            level: LevelName(Level::Info),
            timestamps: true,
            colour: ColourName(ColourMode::Auto),
            categories: BTreeMap::new(),
        }
    }
}

/// The `log` table.
#[derive(Debug, Default)]
struct Log(Fields);

impl<'de> Deserialize<'de> for Log {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `log`").map(Log)
    }
}

/// What the file says about logging, with the defaults filled in.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LogConfig {
    pub(crate) level: Level,
    pub(crate) timestamps: bool,
    pub(crate) colour: ColourMode,
    /// Per-category levels, in name order.
    pub(crate) categories: Vec<(String, Level)>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Log::default().into()
    }
}

impl From<Log> for LogConfig {
    fn from(Log(fields): Log) -> Self {
        Self {
            level: fields.level.0,
            timestamps: fields.timestamps,
            colour: fields.colour.0,
            categories: fields
                .categories
                .into_iter()
                .map(|(name, level)| (name.0, level.0))
                .collect(),
        }
    }
}

impl LogConfig {
    /// The `ck_log` settings this configuration asks for. `is_terminal` and `no_color` (the value of
    /// `NO_COLOR`, `None` when unset) only matter for `colour = "auto"`.
    pub(crate) fn settings(
        &self,
        is_terminal: bool,
        no_color: Option<&OsStr>,
    ) -> Result<Settings, String> {
        let mut filter = Filter::new(self.level);
        for (category, level) in &self.categories {
            filter
                .set_category(category, *level)
                .map_err(|error| error.to_string())?;
        }
        Ok(Settings {
            filter,
            timestamps: self.timestamps,
            colour: self.colour.enabled(is_terminal, no_color),
        })
    }
}

/// The longest an owner deadline may be, in seconds.
const MAX_SECONDS: u64 = 60;

/// A number of seconds, greater than 0 and at most [`MAX_SECONDS`], as a [`Duration`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seconds(Duration);

impl Seconds {
    /// Checks the value *before* it becomes a `Duration`: a NaN, an infinity, zero, a negative number
    /// or a number past the maximum is out of range, and none of them reaches `Duration`'s panicking
    /// constructors.
    fn from_f64(value: f64) -> Option<Self> {
        let in_range = value.is_finite() && value > 0.0 && value <= 60.0;
        in_range
            .then(|| Duration::from_secs_f64(value))
            .filter(|duration| !duration.is_zero())
            .map(Self)
    }

    fn from_u64(value: u64) -> Option<Self> {
        (1..=MAX_SECONDS)
            .contains(&value)
            .then(|| Self(Duration::from_secs(value)))
    }
}

impl<'de> Deserialize<'de> for Seconds {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Number;

        impl Number {
            fn out_of_range<E: de::Error>(shown: impl fmt::Display) -> E {
                E::custom(format!(
                    "{shown} is out of range: expected a number of seconds greater than 0 and at most {MAX_SECONDS}"
                ))
            }
        }

        impl Visitor<'_> for Number {
            type Value = Seconds;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "a number of seconds greater than 0 and at most {MAX_SECONDS}"
                )
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Seconds, E> {
                Seconds::from_u64(value).ok_or_else(|| Self::out_of_range(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Seconds, E> {
                u64::try_from(value)
                    .ok()
                    .and_then(Seconds::from_u64)
                    .ok_or_else(|| Self::out_of_range(value))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Seconds, E> {
                Seconds::from_f64(value).ok_or_else(|| Self::out_of_range(value))
            }
        }

        deserializer.deserialize_any(Number)
    }
}

/// The keys of the `runtime` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RuntimeFields {
    #[serde(rename = "start_timeout")]
    start: Seconds,
    #[serde(rename = "stop_timeout")]
    stop: Seconds,
    #[serde(rename = "cleanup_timeout")]
    cleanup: Seconds,
}

impl Default for RuntimeFields {
    fn default() -> Self {
        Self {
            start: Seconds(Duration::from_secs(5)),
            stop: Seconds(Duration::from_secs(5)),
            cleanup: Seconds(Duration::from_secs(1)),
        }
    }
}

/// The `runtime` table.
#[derive(Debug, Default)]
struct Runtime(RuntimeFields);

impl<'de> Deserialize<'de> for Runtime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `runtime`").map(Runtime)
    }
}

/// The owner deadlines `ck-runtime` supervises the VMs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerDeadlines {
    pub(crate) start: Duration,
    pub(crate) stop: Duration,
    pub(crate) cleanup: Duration,
}

impl Default for OwnerDeadlines {
    fn default() -> Self {
        Runtime::default().into()
    }
}

impl From<Runtime> for OwnerDeadlines {
    fn from(Runtime(fields): Runtime) -> Self {
        Self {
            start: fields.start.0,
            stop: fields.stop.0,
            cleanup: fields.cleanup.0,
        }
    }
}

/// A window backend named in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
struct BackendName(Backend);

/// The text is not a backend name.
#[derive(Debug)]
struct UnknownBackend(String);

impl fmt::Display for UnknownBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown backend {:?}: expected {}",
            self.0,
            Backend::NAMES
        )
    }
}

impl TryFrom<String> for BackendName {
    type Error = UnknownBackend;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Backend::from_name(&text)
            .map(Self)
            .ok_or(UnknownBackend(text))
    }
}

/// The keys of the `window` table: one optional key.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct WindowFields {
    backend: Option<BackendName>,
}

/// The `window` table.
#[derive(Debug, Default)]
struct Window(WindowFields);

impl<'de> Deserialize<'de> for Window {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `window`").map(Window)
    }
}

/// `[input.bindings]`: action names to arrays of binding strings (design §7.1). A value that is not an array of
/// strings is a configuration error; what a binding string may say is `ck-input`'s rule.
#[derive(Debug, Default)]
struct BindingsTable(BTreeMap<String, Vec<String>>);

impl<'de> Deserialize<'de> for BindingsTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `input.bindings`").map(Self)
    }
}

/// The keys of the `input` table: one sub-table, `bindings`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct InputFields {
    bindings: Option<BindingsTable>,
}

/// The `input` table.
#[derive(Debug, Default)]
struct InputTable(InputFields);

impl<'de> Deserialize<'de> for InputTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `input`").map(InputTable)
    }
}

/// The default `[frame]` rate, in updates per second (design §7.1).
const DEFAULT_UPDATES_PER_SECOND: u32 = 60;
/// The most updates per second `[frame]` allows.
const MAX_UPDATES_PER_SECOND: u32 = 240;
/// The default `[frame]` largest delivered delta, in seconds (design §7.1).
const DEFAULT_MAX_DELTA_SECONDS: f64 = 0.1;
/// The largest `max_delta_seconds` `[frame]` allows.
const MAX_DELTA_SECONDS: f64 = 1.0;

/// `updates_per_second`: an integer from 1 to [`MAX_UPDATES_PER_SECOND`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rate(u32);

impl<'de> Deserialize<'de> for Rate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Number;

        impl Number {
            fn refused<E: de::Error>(shown: impl fmt::Display) -> E {
                E::custom(format!(
                    "updates_per_second {shown} is out of range: expected an integer from 1 to {MAX_UPDATES_PER_SECOND}"
                ))
            }
        }

        impl Visitor<'_> for Number {
            type Value = Rate;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "updates_per_second as an integer from 1 to {MAX_UPDATES_PER_SECOND}"
                )
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Rate, E> {
                u32::try_from(value)
                    .ok()
                    .filter(|rate| (1..=MAX_UPDATES_PER_SECOND).contains(rate))
                    .map(Rate)
                    .ok_or_else(|| Self::refused(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Rate, E> {
                u64::try_from(value)
                    .map_err(|_| Self::refused(value))
                    .and_then(|value| self.visit_u64(value))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Rate, E> {
                // A float is not an integer rate, whatever its value.
                Err(E::custom(format!(
                    "updates_per_second {value} must be an integer from 1 to {MAX_UPDATES_PER_SECOND}, not a float"
                )))
            }
        }

        deserializer.deserialize_any(Number)
    }
}

/// `max_delta_seconds`: a finite number from [`MIN_DELTA_SECONDS`] (one nanosecond, the timer's resolution) to
/// [`MAX_DELTA_SECONDS`]. A positive value below the minimum is refused **before** it becomes a `Duration`, so the limit
/// means what it says and does not depend on how a float rounds to a duration (a value just under one nanosecond would
/// round up to it, one under half of it down to zero).
#[derive(Debug, Clone, Copy, PartialEq)]
struct MaxDelta(Duration);

/// The smallest `max_delta_seconds` as a timer duration: one nanosecond.
const MIN_DELTA_SECONDS: f64 = 1e-9;

/// Why a `max_delta_seconds` is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaxDeltaRefusal {
    /// Not finite, not greater than 0, or greater than [`MAX_DELTA_SECONDS`].
    Range,
    /// Greater than 0 but below [`MIN_DELTA_SECONDS`], the timer's resolution.
    BelowResolution,
}

impl MaxDelta {
    fn from_f64(value: f64) -> Result<Self, MaxDeltaRefusal> {
        if !(value.is_finite() && value > 0.0 && value <= MAX_DELTA_SECONDS) {
            return Err(MaxDeltaRefusal::Range);
        }
        if value < MIN_DELTA_SECONDS {
            return Err(MaxDeltaRefusal::BelowResolution);
        }
        Ok(Self(Duration::from_secs_f64(value)))
    }
}

impl<'de> Deserialize<'de> for MaxDelta {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Number;

        impl Number {
            fn refused<E: de::Error>(shown: impl fmt::Display) -> E {
                E::custom(format!(
                    "max_delta_seconds {shown} is out of range: expected a number greater than 0 and at most {MAX_DELTA_SECONDS}"
                ))
            }

            fn refused_by<E: de::Error>(shown: f64, why: MaxDeltaRefusal) -> E {
                match why {
                    MaxDeltaRefusal::Range => Self::refused(shown),
                    MaxDeltaRefusal::BelowResolution => E::custom(format!(
                        "max_delta_seconds {shown} is below the timer resolution: the smallest accepted value is {MIN_DELTA_SECONDS} (one nanosecond)"
                    )),
                }
            }
        }

        impl Visitor<'_> for Number {
            type Value = MaxDelta;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "max_delta_seconds as a number greater than 0 and at most {MAX_DELTA_SECONDS}"
                )
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<MaxDelta, E> {
                // Only 1 is in range; anything larger is refused by the range itself.
                if value == 1 {
                    MaxDelta::from_f64(1.0).map_err(|why| Self::refused_by(1.0, why))
                } else {
                    Err(Self::refused(value))
                }
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<MaxDelta, E> {
                u64::try_from(value)
                    .map_err(|_| Self::refused(value))
                    .and_then(|value| self.visit_u64(value))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<MaxDelta, E> {
                MaxDelta::from_f64(value).map_err(|why| Self::refused_by(value, why))
            }
        }

        deserializer.deserialize_any(Number)
    }
}

/// The keys of the `frame` table (design §7.1).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct FrameFields {
    updates_per_second: Option<Rate>,
    max_delta_seconds: Option<MaxDelta>,
}

/// The `frame` table.
#[derive(Debug, Default)]
struct FrameTable(FrameFields);

impl<'de> Deserialize<'de> for FrameTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `frame`").map(FrameTable)
    }
}

/// What the file says about the paced UI update, with the defaults filled in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameConfig {
    /// The target rate, 1 to 240.
    pub(crate) updates_per_second: u32,
    /// The largest delta a callback is given, greater than 0 and at most one second.
    pub(crate) max_delta: Duration,
}

impl Default for FrameConfig {
    fn default() -> Self {
        Self {
            updates_per_second: DEFAULT_UPDATES_PER_SECOND,
            max_delta: Duration::from_secs_f64(DEFAULT_MAX_DELTA_SECONDS),
        }
    }
}

impl FrameConfig {
    /// The loop's pacing settings: a new update source, **off** (no update is posted, and no timed wake happens,
    /// until the control is turned on).
    ///
    /// # Errors
    ///
    /// Never for a value read from a file (the ranges are inside the loop's own); the error is the loop's.
    pub(crate) fn update_settings(&self) -> Result<UpdateSettings, SettingsError> {
        UpdateSettings::new(self.updates_per_second, self.max_delta)
    }
}

/// The keys of the `ui` table: `scale`, the highest UI scale to use (1, 1.5 or 2; the scale in use is the largest the window allows, or
/// this when it is lower), read at startup only.
///
/// `points_per_pixel` and `font_points_per_pixel` are accepted only to be refused by name, so any value of either is read here.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct UiFields {
    scale: Option<toml::Value>,
    points_per_pixel: Option<toml::Value>,
    font_points_per_pixel: Option<toml::Value>,
    tooltips: Option<toml::Value>,
}

/// The `ui` table.
#[derive(Debug, Default)]
struct UiTable(UiFields);

impl<'de> Deserialize<'de> for UiTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `ui`").map(UiTable)
    }
}

/// A sampling filter named in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
struct FilterName(ImageFilter);

impl TryFrom<String> for FilterName {
    type Error = ck_render_2d::UnknownFilter;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse().map(Self)
    }
}

/// The keys of the `render` table (design §6): the sampling filter, assets directory and scene cap.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RenderFields {
    filter: Option<FilterName>,
    assets: Option<String>,
    max_quads: Option<u64>,
    max_groups: Option<u64>,
    bindless_texture_slots: Option<u64>,
}

/// The `render` table.
#[derive(Debug, Default)]
struct RenderTable(RenderFields);

impl<'de> Deserialize<'de> for RenderTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `render`").map(RenderTable)
    }
}

/// What the file says about rendering, with the defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderConfig {
    /// How images are sampled (`nearest` unless the file says `linear`).
    pub(crate) filter: ImageFilter,
    /// The assets directory: the file's `assets` (default `assets`) relative to the configuration file's
    /// directory. `None` without a configuration file, where there is no directory to be relative to; the first
    /// `render.quad` then raises an error saying so.
    pub(crate) assets: Option<PathBuf>,
    /// Maximum quads in one scene.
    pub(crate) max_quads: u32,
    /// Maximum groups in one scene, the base group not counted (milestone 8, slice 4).
    pub(crate) max_groups: u32,
    /// The entry count of the bindless texture tables (design §7.1): at least `max_textures + max_extra_revisions + 1` (the
    /// checker's slot 0), checked at configuration time against the `[resources]` values.
    pub(crate) bindless_texture_slots: u32,
}

/// The default `[resources] max_textures` of the application: the milestone 7 application uses about a hundred textures (the pixel
/// font's glyphs, the panel, the checker and the map), so 2,048 is about twenty times what it needs. The resource service's own
/// default (65,536) is for the scale runs, which set their limits explicitly.
pub(crate) const DEFAULT_MAX_TEXTURES: u32 = 2_048;

/// The default `[resources] max_extra_revisions` (design §7.1).
pub(crate) const DEFAULT_MAX_EXTRA_REVISIONS: usize = 1_024;

/// The default `bindless_texture_slots` (design §7.1): it covers `max_textures + max_extra_revisions + 1` (3,073) with room to
/// spare. The size is a cost, not only a capacity: `MoltenVK` walks the whole table at every scene draw (about 20 ns a slot), so the
/// former 131,072-slot default cost 2.8 ms of encode a frame on `glfw-vulkan` against 0.28 ms for 4,096 (CK-201). A larger table
/// is configured with `[render] bindless_texture_slots` and the `[resources]` limits it must cover.
pub(crate) const DEFAULT_BINDLESS_TEXTURE_SLOTS: u32 = 4_096;

/// The bindless table's slot range (the backend's `BindlessLimits::validate`; this application cannot import it without
/// depending on the Metal backend crate, so the two are kept aligned by hand and by a test that parses both ends).
const BINDLESS_TEXTURE_SLOTS: std::ops::RangeInclusive<u64> = 2..=1_000_000;

/// Checks `[render] bindless_texture_slots` against the range and the §7.1 rule, without overflow; the refusal names the
/// three numbers.
fn bindless_slots(
    path: &Path,
    requested: Option<u64>,
    resources: &ResourceConfig,
) -> Result<u32, String> {
    let slots = requested.unwrap_or(u64::from(DEFAULT_BINDLESS_TEXTURE_SLOTS));
    if !BINDLESS_TEXTURE_SLOTS.contains(&slots) {
        return Err(format!(
            "{}: [render]: bindless_texture_slots={slots} is outside 2..=1000000",
            path.display()
        ));
    }
    let max_textures = resources.service.max_textures;
    let max_extra = u32::try_from(resources.max_extra_revisions).ok();
    let required = max_extra
        .and_then(|extra| max_textures.checked_add(extra))
        .and_then(|sum| sum.checked_add(1));
    match required {
        Some(required) if slots >= u64::from(required) => u32::try_from(slots).map_err(|_| {
            format!(
                "{}: [render]: bindless_texture_slots={slots} does not fit u32",
                path.display()
            )
        }),
        Some(required) => Err(format!(
            "{}: [render]: bindless_texture_slots={slots} is below max_textures={max_textures} + max_extra_revisions={} + 1 \
             (the checker's slot) = {required}",
            path.display(),
            resources.max_extra_revisions
        )),
        None => Err(format!(
            "{}: [render]: max_textures={max_textures} + max_extra_revisions={} + 1 overflows u32, so no \
             bindless_texture_slots={slots} can cover it",
            path.display(),
            resources.max_extra_revisions
        )),
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            filter: ImageFilter::Nearest,
            assets: None,
            max_quads: SceneLimits::DEFAULT.max_quads,
            max_groups: SceneLimits::DEFAULT.max_groups,
            bindless_texture_slots: DEFAULT_BINDLESS_TEXTURE_SLOTS,
        }
    }
}

/// Optional resource-service and backend limits from `[resources]`.
#[derive(Debug, Default, Deserialize, Copy, Clone)]
#[serde(deny_unknown_fields, default)]
struct ResourceFields {
    max_textures: Option<u64>,
    max_pending_jobs: Option<u64>,
    decode_workers: Option<u64>,
    cpu_pixel_bytes: Option<u64>,
    metadata_bytes: Option<u64>,
    // Keep these ranges and cross-field rules aligned with the backend TextureLimits::validate from CK-112;
    // this application cannot import that validator without depending on the Metal backend crate.
    max_prepared_images: Option<u64>,
    max_upload_images_per_batch: Option<u64>,
    max_upload_bytes_per_batch: Option<u64>,
    staging_bytes: Option<u64>,
    texture_bytes: Option<u64>,
    max_extra_revisions: Option<u64>,
    max_adoptions_per_frame: Option<u64>,
    binding_geometry_bytes: Option<u64>,
}

#[derive(Debug, Default)]
struct ResourceTable(ResourceFields);

impl<'de> Deserialize<'de> for ResourceTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `resources`").map(ResourceTable)
    }
}

/// Limits configured for the resource service and later native backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResourceConfig {
    pub(crate) service: ResourceLimits,
    pub(crate) max_prepared_images: usize,
    pub(crate) max_upload_images_per_batch: usize,
    pub(crate) max_upload_bytes_per_batch: u64,
    pub(crate) staging_bytes: u64,
    pub(crate) texture_bytes: u64,
    pub(crate) max_extra_revisions: usize,
    pub(crate) max_adoptions_per_frame: usize,
    pub(crate) binding_geometry_bytes: u64,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            service: ResourceLimits {
                max_textures: DEFAULT_MAX_TEXTURES,
                ..ResourceLimits::DEFAULT
            },
            max_prepared_images: 256,
            max_upload_images_per_batch: 128,
            max_upload_bytes_per_batch: 16 * 1024 * 1024,
            staging_bytes: 128 * 1024 * 1024,
            texture_bytes: 2 * 1024 * 1024 * 1024,
            max_extra_revisions: DEFAULT_MAX_EXTRA_REVISIONS,
            max_adoptions_per_frame: 256,
            binding_geometry_bytes: 128 * 1024 * 1024,
        }
    }
}

impl ResourceConfig {
    fn parse(fields: ResourceFields) -> Result<Self, String> {
        let defaults = Self::default();
        let config = Self {
            service: ResourceLimits {
                max_textures: u32_value(
                    "max_textures",
                    fields.max_textures,
                    u64::from(defaults.service.max_textures),
                    1,
                    1_048_576,
                )?,
                max_pending_jobs: u32_value(
                    "max_pending_jobs",
                    fields.max_pending_jobs,
                    u64::from(defaults.service.max_pending_jobs),
                    1,
                    1_048_576,
                )?,
                decode_workers: u32_value(
                    "decode_workers",
                    fields.decode_workers,
                    u64::from(defaults.service.decode_workers),
                    1,
                    16,
                )?,
                cpu_pixel_bytes: bounded(
                    "cpu_pixel_bytes",
                    fields.cpu_pixel_bytes,
                    defaults.service.cpu_pixel_bytes,
                    64 << 20,
                    64 << 30,
                )?,
                metadata_bytes: bounded(
                    "metadata_bytes",
                    fields.metadata_bytes,
                    defaults.service.metadata_bytes,
                    1 << 20,
                    16 << 30,
                )?,
            },
            max_prepared_images: usize_value(
                "max_prepared_images",
                fields.max_prepared_images,
                defaults.max_prepared_images as u64,
                2,
                4096,
            )?,
            max_upload_images_per_batch: usize_value(
                "max_upload_images_per_batch",
                fields.max_upload_images_per_batch,
                defaults.max_upload_images_per_batch as u64,
                1,
                2048,
            )?,
            max_upload_bytes_per_batch: bounded(
                "max_upload_bytes_per_batch",
                fields.max_upload_bytes_per_batch,
                defaults.max_upload_bytes_per_batch,
                1 << 20,
                256 << 20,
            )?,
            staging_bytes: bounded(
                "staging_bytes",
                fields.staging_bytes,
                defaults.staging_bytes,
                128 << 20,
                4 << 30,
            )?,
            texture_bytes: bounded(
                "texture_bytes",
                fields.texture_bytes,
                defaults.texture_bytes,
                64 << 20,
                64 << 30,
            )?,
            max_extra_revisions: usize_value(
                "max_extra_revisions",
                fields.max_extra_revisions,
                defaults.max_extra_revisions as u64,
                1,
                65_536,
            )?,
            max_adoptions_per_frame: usize_value(
                "max_adoptions_per_frame",
                fields.max_adoptions_per_frame,
                defaults.max_adoptions_per_frame as u64,
                1,
                65_536,
            )?,
            binding_geometry_bytes: bounded(
                "binding_geometry_bytes",
                fields.binding_geometry_bytes,
                defaults.binding_geometry_bytes,
                1 << 20,
                4 << 30,
            )?,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.max_prepared_images < self.max_upload_images_per_batch.saturating_mul(2) {
            return Err(format!(
                "[resources]: max_prepared_images={} must be at least twice max_upload_images_per_batch={}",
                self.max_prepared_images, self.max_upload_images_per_batch
            ));
        }
        if self.max_upload_bytes_per_batch > self.staging_bytes / 2 {
            return Err(format!(
                "[resources]: max_upload_bytes_per_batch={} exceeds staging_bytes/2={}",
                self.max_upload_bytes_per_batch,
                self.staging_bytes / 2
            ));
        }
        self.service
            .validate()
            .map_err(|error| format!("[resources]: {error}"))?;
        Ok(())
    }
}

fn bounded(
    name: &str,
    value: Option<u64>,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let value = value.unwrap_or(default);
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "[resources]: {name}={value} is outside {min}..={max}"
        ))
    }
}

fn u32_value(
    name: &str,
    value: Option<u64>,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u32, String> {
    u32::try_from(bounded(name, value, default, min, max)?)
        .map_err(|_| format!("[resources]: {name} does not fit u32"))
}

fn usize_value(
    name: &str,
    value: Option<u64>,
    default: u64,
    min: u64,
    max: u64,
) -> Result<usize, String> {
    usize::try_from(bounded(name, value, default, min, max)?)
        .map_err(|_| format!("[resources]: {name} does not fit usize"))
}

/// The directory `assets` names when it is not given.
const DEFAULT_ASSETS: &str = "assets";

/// The name of a VM: a key of the `lua` table. It follows `ck-lua`'s rule: lowercase letters, digits
/// and `_`, and not empty.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
struct VmName(String);

/// The text is not a VM name.
#[derive(Debug)]
struct InvalidVmName(String);

impl fmt::Display for InvalidVmName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid VM name {:?}: use lowercase letters, digits and `_`",
            self.0
        )
    }
}

impl TryFrom<String> for VmName {
    type Error = InvalidVmName;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let valid = !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if valid {
            Ok(Self(text))
        } else {
            Err(InvalidVmName(text))
        }
    }
}

/// The keys of a `lua.<name>` table: exactly one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VmFields {
    entry: String,
}

/// A `lua.<name>` table.
#[derive(Debug)]
struct Vm(VmFields);

impl<'de> Deserialize<'de> for Vm {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        table_only(deserializer, "a table for `lua.<name>`").map(Vm)
    }
}

/// One configured VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VmConfig {
    pub(crate) name: String,
    /// The entry script: absolute or relative as written when absolute, otherwise joined to the
    /// directory of the configuration file.
    pub(crate) entry: PathBuf,
}

/// Everything the file says: logging, the owner deadlines, the VMs and the whole file as a value.
#[derive(Debug, PartialEq)]
pub(crate) struct Config {
    /// `[ui] scale` as `k` (2, 3 or 4), the limit on the UI scale of a window opened in screen pixels, if the file sets one.
    pub(crate) ui_scale_limit_k: Option<u32>,
    /// Startup-only tooltip settings.
    pub(crate) tooltips: TooltipsConfig,
    /// Effective `[map]` controls, including settings-file overrides even when no map is present.
    pub(crate) map_settings: MapSettings,
    /// The settings file is anchored to the configured saves directory; `None` means this run had no `--config`.
    pub(crate) settings_file: Option<PathBuf>,
    /// Values the settings file overrides, retained so a later set/reset writes only those overrides.
    pub(crate) settings_overrides: SettingsOverrides,
    /// The configuration-file values to which a tab reset returns.
    pub(crate) settings_base_tooltips: TooltipsConfig,
    pub(crate) settings_base_map: MapSettings,
    /// Startup settings-file warnings, emitted after the configured logger is installed.
    pub(crate) settings_warnings: Vec<String>,
    pub(crate) log: LogConfig,
    pub(crate) deadlines: OwnerDeadlines,
    /// The VMs in name order (they start in this order and stop in reverse).
    pub(crate) vms: Vec<VmConfig>,
    /// The `[window]` table's backend, if the file names one: none means a headless run.
    pub(crate) window_backend: Option<Backend>,
    /// Optional map input and camera controls. The path is relative to the configuration file. With `[game] scenario` the
    /// map is the scenario's, and the directory is the scenario directory.
    pub(crate) map: Option<MapConfig>,
    /// The `[game]` table: the simulation's scenario and quicksave directory. Without it there is no simulation.
    pub(crate) game: Option<GameConfig>,
    /// Resource-service and backend limits, with defaults filled in.
    pub(crate) resources: ResourceConfig,
    /// The `[render]` table, with its defaults.
    pub(crate) render: RenderConfig,
    /// The `[frame]` table, with its defaults.
    pub(crate) frame: FrameConfig,
    /// The `[input.bindings]` table, validated, with the defaults of design §7.1 filling every action the file does
    /// not name (an explicit empty list unbinds). Read-only after load, like the other settings.
    pub(crate) bindings: Bindings,
    /// The whole file, which every VM sees as its `config`.
    pub(crate) whole: ConfigValue,
}

/// Startup-only tooltip delay and lock policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TooltipsConfig {
    pub(crate) delay: f64,
    pub(crate) lock: &'static str,
    pub(crate) lock_timer: f64,
}

/// The three settings that control map input, whether or not a map is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapSettings {
    pub(crate) pan_pixels_per_frame: u32,
    pub(crate) zoom_steps_per_second: u32,
    pub(crate) scroll_points_per_step: u32,
}

impl Default for MapSettings {
    fn default() -> Self {
        Self {
            pan_pixels_per_frame: 4,
            zoom_steps_per_second: 20,
            scroll_points_per_step: 24,
        }
    }
}

/// Validated values present in `settings.toml`; absent keys inherit their config/default values.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct SettingsOverrides {
    pub(crate) tooltip_delay: Option<f64>,
    pub(crate) tooltip_lock: Option<&'static str>,
    pub(crate) tooltip_lock_timer: Option<f64>,
    pub(crate) map_pan_pixels_per_frame: Option<u32>,
    pub(crate) map_zoom_steps_per_second: Option<u32>,
    pub(crate) map_scroll_points_per_step: Option<u32>,
}

impl Default for TooltipsConfig {
    fn default() -> Self {
        Self {
            delay: 0.3,
            lock: "timer",
            lock_timer: 1.0,
        }
    }
}

impl TooltipsConfig {
    fn value(self) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            ("delay".to_owned(), ConfigValue::Float(self.delay)),
            ("lock".to_owned(), ConfigValue::String(self.lock.to_owned())),
            ("lock_timer".to_owned(), ConfigValue::Float(self.lock_timer)),
        ]))
    }

    /// The read-only `settings.tooltips()` extension.
    pub(crate) fn extension(self) -> ck_lua::Extension {
        use std::sync::Arc;
        ck_lua::Extension::new("settings").function(
            "tooltips",
            Arc::new(move |arguments| {
                if !arguments.is_empty() {
                    return Err("settings.tooltips: expected no arguments".to_owned());
                }
                Ok(vec![self.value()])
            }),
        )
    }
}

/// Validated map configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapConfig {
    pub(crate) directory: PathBuf,
    pub(crate) pan_pixels_per_frame: u32,
    pub(crate) zoom_steps_per_second: u32,
    pub(crate) scroll_points_per_step: u32,
}

/// Validated `[game]` configuration. Both paths are relative to the configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GameConfig {
    /// The scenario directory.
    pub(crate) scenario: PathBuf,
    /// The quicksave directory.
    pub(crate) saves: PathBuf,
}

/// The default quicksave directory, beside the configuration file.
const DEFAULT_SAVES: &str = "saves";

/// The time and save actions with their default keys, added only when `[game] scenario` is set.
const GAME_BINDINGS: [(&str, &str); 10] = [
    ("time_pause", "Space"),
    ("time_speed_1", "Digit1"),
    ("time_speed_2", "Digit2"),
    ("time_speed_3", "Digit3"),
    ("time_speed_4", "Digit4"),
    ("time_speed_5", "Digit5"),
    ("time_faster", "BracketRight"),
    ("time_slower", "BracketLeft"),
    ("game_quicksave", "F5"),
    ("game_quickload", "F9"),
];

impl Default for Config {
    /// The built-in defaults: the same as an empty file, so no VMs.
    fn default() -> Self {
        Self {
            ui_scale_limit_k: None,
            tooltips: TooltipsConfig::default(),
            map_settings: MapSettings::default(),
            settings_file: None,
            settings_overrides: SettingsOverrides::default(),
            settings_base_tooltips: TooltipsConfig::default(),
            settings_base_map: MapSettings::default(),
            settings_warnings: Vec::new(),
            log: LogConfig::default(),
            deadlines: OwnerDeadlines::default(),
            vms: Vec::new(),
            window_backend: None,
            map: None,
            game: None,
            resources: ResourceConfig::default(),
            render: RenderConfig::default(),
            frame: FrameConfig::default(),
            bindings: ck_window::LoopSettings::default().bindings().clone(),
            whole: ConfigValue::Table(BTreeMap::new()),
        }
    }
}

/// The value of `value` as a [`ConfigValue`]. A TOML date or time becomes its TOML string form.
fn to_config_value(value: &toml::Value) -> ConfigValue {
    match value {
        toml::Value::String(text) => ConfigValue::String(text.clone()),
        toml::Value::Integer(number) => ConfigValue::Integer(*number),
        toml::Value::Float(number) => ConfigValue::Float(*number),
        toml::Value::Boolean(flag) => ConfigValue::Bool(*flag),
        toml::Value::Datetime(moment) => ConfigValue::String(moment.to_string()),
        toml::Value::Array(items) => {
            ConfigValue::Array(items.iter().map(to_config_value).collect())
        }
        toml::Value::Table(table) => ConfigValue::Table(
            table
                .iter()
                .map(|(key, item)| (key.clone(), to_config_value(item)))
                .collect(),
        ),
    }
}

/// Reads and parses the file at `path`. The error is the message of the `config` record: it names the
/// path, and for a problem in the text, the 1-based line and the offending key or value.
pub(crate) fn load(path: &Path) -> Result<Config, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut config = parse(path, &text)?;
    load_settings(path, &mut config);
    Ok(config)
}

/// The shipped-config test keeps its fixture independent of a player's `settings.toml` in the copy root.
#[cfg(test)]
fn load_without_settings(path: &Path) -> Result<Config, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(path, &text)
}

/// The message for a TOML error in `text`, with its 1-based line when it has a span.
fn describe(path: &Path, text: &str, error: &toml::de::Error) -> String {
    let message = error.message().replace('\n', " ");
    match error.span() {
        Some(span) => {
            let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
            format!("{}: line {line}: {message}", path.display())
        }
        None => format!("{}: {message}", path.display()),
    }
}

/// The configuration `text` says, for another module's test.
#[cfg(test)]
pub(crate) fn load_text_for_test(text: &str) -> Config {
    parse(Path::new("/tmp/codekarl-test/config.toml"), text).expect("a valid configuration")
}

/// The `k` of a `[ui] scale` (TOML integer or float: 1, 1.5 or 2), or the refusal naming the accepted values.
fn ui_scale_k(path: &Path, value: &toml::Value) -> Result<u32, String> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "only 1 and 2 are accepted: any other integer is refused below whatever its rounding"
    )]
    let number = match value {
        toml::Value::Integer(integer) => Some(*integer as f64),
        toml::Value::Float(float) => Some(*float),
        _ => None,
    };
    number
        .and_then(ck_render_2d::ui_scale::k_of_scale)
        .ok_or_else(|| {
            format!(
                "{}: [ui]: scale = {value} is not accepted: use 1, 1.5 or 2",
                path.display()
            )
        })
}

/// The pixel font's and the UI scale's checks: the old `[ui] font_points_per_pixel` key is refused, naming `[ui] scale`, even beside the refused
/// `[ui] points_per_pixel`; `[ui] points_per_pixel` is retired and refused, naming `[ui] scale`; `[ui] scale` is 1, 1.5 or 2; and with a game the scene's one sampling filter is
/// `nearest` (the glyphs need it, and the scene has no per-quad filter).
fn check_pixel_font(
    path: &Path,
    ui: &UiFields,
    has_game: bool,
    filter: Option<FilterName>,
) -> Result<(), String> {
    if ui.font_points_per_pixel.is_some() {
        return Err(format!(
            "{}: [ui]: font_points_per_pixel was renamed: use scale (1, 1.5 or 2)",
            path.display()
        ));
    }
    if ui.points_per_pixel.is_some() {
        return Err(format!(
            "{}: [ui]: points_per_pixel is retired: the UI scale follows the window; use scale (1, 1.5 or 2) to limit it",
            path.display()
        ));
    }
    if let Some(scale) = &ui.scale {
        ui_scale_k(path, scale)?;
    }
    if has_game && filter.is_some_and(|name| name.0 == ImageFilter::Linear) {
        return Err(format!(
            "{}: [render] filter = \"linear\" cannot be used with [game] scenario: the pixel font needs nearest sampling",
            path.display()
        ));
    }
    Ok(())
}

/// The scene's two capacities from `[render]`: `max_quads` (1 to 1,048,576) and `max_groups` (1 to 65,536), each with its default.
fn scene_capacities(path: &Path, render: &RenderFields) -> Result<(u32, u32), String> {
    let max_quads = render
        .max_quads
        .unwrap_or(u64::from(SceneLimits::DEFAULT.max_quads));
    if !(1..=1_048_576).contains(&max_quads) {
        return Err(format!(
            "{}: [render]: max_quads={max_quads} is outside 1..=1048576",
            path.display()
        ));
    }
    let max_quads = u32::try_from(max_quads)
        .map_err(|_| format!("{}: [render]: max_quads does not fit u32", path.display()))?;
    let max_groups = render
        .max_groups
        .unwrap_or(u64::from(SceneLimits::DEFAULT.max_groups));
    if !(1..=65_536).contains(&max_groups) {
        return Err(format!(
            "{}: [render]: max_groups={max_groups} is outside 1..=65536",
            path.display()
        ));
    }
    let max_groups = u32::try_from(max_groups)
        .map_err(|_| format!("{}: [render]: max_groups does not fit u32", path.display()))?;
    Ok((max_quads, max_groups))
}

#[expect(
    clippy::too_many_lines,
    reason = "one strict parse pass validates the source tables before building the effective config"
)]
fn parse(path: &Path, text: &str) -> Result<Config, String> {
    let file = toml::from_str::<File>(text).map_err(|error| describe(path, text, &error))?;
    let whole = text
        .parse::<toml::Table>()
        .map_err(|error| describe(path, text, &error))?;
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    // The entries go to `ck-input`, whose refusal fails the load with its message naming the action and the entry.
    let game = parse_game(path, base, file.game)?;
    let map = parse_map(path, base, file.map, game.as_ref())?;
    let bindings = if map.is_some() {
        let mut entries = BTreeMap::from([
            ("map_pan_left".to_owned(), vec!["ArrowLeft".to_owned()]),
            ("map_pan_right".to_owned(), vec!["ArrowRight".to_owned()]),
            ("map_pan_up".to_owned(), vec!["ArrowUp".to_owned()]),
            ("map_pan_down".to_owned(), vec!["ArrowDown".to_owned()]),
            (
                "map_zoom_in".to_owned(),
                vec!["Equal".to_owned(), "Shift+Equal".to_owned()],
            ),
            ("map_zoom_out".to_owned(), vec!["Minus".to_owned()]),
        ]);
        if game.is_some() {
            entries.extend(
                GAME_BINDINGS
                    .iter()
                    .map(|(action, key)| ((*action).to_owned(), vec![(*key).to_owned()])),
            );
        }
        if let Some(BindingsTable(custom)) = file.input.0.bindings {
            entries.extend(custom);
        }
        Bindings::from_entries(entries)
            .map_err(|error| format!("{}: [input.bindings]: {error}", path.display()))?
    } else {
        match file.input.0.bindings {
            None => ck_window::LoopSettings::default().bindings().clone(),
            Some(BindingsTable(entries)) => Bindings::from_entries(entries)
                .map_err(|error| format!("{}: [input.bindings]: {error}", path.display()))?,
        }
    };
    check_pixel_font(path, &file.ui.0, game.is_some(), file.render.0.filter)?;
    let tooltips = parse_tooltips(path, file.ui.0.tooltips.as_ref())?;
    let map_settings = map
        .as_ref()
        .map_or_else(MapSettings::default, |map| MapSettings {
            pan_pixels_per_frame: map.pan_pixels_per_frame,
            zoom_steps_per_second: map.zoom_steps_per_second,
            scroll_points_per_step: map.scroll_points_per_step,
        });
    let ui_scale_limit_k = file
        .ui
        .0
        .scale
        .as_ref()
        .map(|scale| ui_scale_k(path, scale))
        .transpose()?;
    let resources = ResourceConfig::parse(file.resources.0)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let (max_quads, max_groups) = scene_capacities(path, &file.render.0)?;
    let bindless_texture_slots =
        bindless_slots(path, file.render.0.bindless_texture_slots, &resources)?;
    let vms = file
        .lua
        .into_iter()
        .map(|(name, Vm(fields))| VmConfig {
            name: name.0,
            entry: base.join(fields.entry),
        })
        .collect();
    Ok(Config {
        ui_scale_limit_k,
        tooltips,
        map_settings,
        settings_file: None,
        settings_overrides: SettingsOverrides::default(),
        settings_base_tooltips: tooltips,
        settings_base_map: map_settings,
        settings_warnings: Vec::new(),
        log: file.log.into(),
        deadlines: file.runtime.into(),
        vms,
        window_backend: file.window.0.backend.map(|name| name.0),
        map,
        game,
        resources,
        render: RenderConfig {
            filter: file
                .render
                .0
                .filter
                .map_or(ImageFilter::Nearest, |name| name.0),
            assets: Some(base.join(file.render.0.assets.as_deref().unwrap_or(DEFAULT_ASSETS))),
            max_quads,
            max_groups,
            bindless_texture_slots,
        },
        frame: FrameConfig {
            updates_per_second: file
                .frame
                .0
                .updates_per_second
                .map_or(DEFAULT_UPDATES_PER_SECOND, |rate| rate.0),
            max_delta: file
                .frame
                .0
                .max_delta_seconds
                .map_or_else(|| FrameConfig::default().max_delta, |delta| delta.0),
        },
        bindings,
        whole: to_config_value(&toml::Value::Table(whole)),
    })
}

/// Validates `[ui.tooltips]`; integer tenths avoid binary-float step errors in the TOML values.
fn parse_tooltips(path: &Path, raw: Option<&toml::Value>) -> Result<TooltipsConfig, String> {
    let mut result = TooltipsConfig::default();
    let Some(raw) = raw else { return Ok(result) };
    let Some(table) = raw.as_table() else {
        return Err(format!("{}: [ui.tooltips] must be a table", path.display()));
    };
    for (key, value) in table {
        match key.as_str() {
            "delay" => result.delay = tooltip_seconds(path, key, value, 0, 20)?,
            "lock_timer" => result.lock_timer = tooltip_seconds(path, key, value, 3, 30)?,
            "lock" => match value.as_str() {
                Some("timer") => result.lock = "timer",
                Some("middle_mouse") => result.lock = "middle_mouse",
                Some("disabled") => result.lock = "disabled",
                _ => {
                    return Err(format!(
                        "{}: [ui.tooltips] lock = {value} is not accepted: use \"timer\", \"middle_mouse\" or \"disabled\"",
                        path.display()
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "{}: [ui.tooltips] unknown key `{key}`; accepted keys are delay, lock and lock_timer",
                    path.display()
                ));
            }
        }
    }
    Ok(result)
}

fn tooltip_seconds(
    path: &Path,
    key: &str,
    value: &toml::Value,
    min_tenths: u32,
    max_tenths: u32,
) -> Result<f64, String> {
    let number = match value {
        toml::Value::Integer(value) => u32::try_from(*value)
            .ok()
            .filter(|value| *value <= 3)
            .map_or(f64::NAN, f64::from),
        toml::Value::Float(value) => *value,
        _ => f64::NAN,
    };
    checked_tooltip_tenths(number, min_tenths, max_tenths)
        .map(|tenths| f64::from(tenths) / 10.0)
        .ok_or_else(|| {
            let accepted = if key == "delay" {
                "0..=2 in 0.1-second steps"
            } else {
                "0.3..=3 in 0.1-second steps"
            };
            format!(
                "{}: [ui.tooltips] {key} = {value} is not accepted: use {accepted}",
                path.display()
            )
        })
}

/// Applies the shared tooltip range and tenth-step rule, returning the canonical rounded value.
pub(crate) fn checked_tooltip_tenths(number: f64, min_tenths: u32, max_tenths: u32) -> Option<u32> {
    let tenths = (number * 10.0).round();
    if number.is_finite()
        && (number * 10.0 - tenths).abs() < 1.0e-9
        && (f64::from(min_tenths)..=f64::from(max_tenths)).contains(&tenths)
    {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the checked range is an integral number of tenths within u32"
        )]
        #[expect(
            clippy::cast_sign_loss,
            reason = "the checked tooltip range starts at zero and remains nonnegative"
        )]
        let tenths = tenths as u32;
        Some(tenths)
    } else {
        None
    }
}

/// `[game]`: the scenario directory and the quicksave directory, both joined to the configuration file's directory.
fn parse_game(
    path: &Path,
    base: &Path,
    game: Option<GameTable>,
) -> Result<Option<GameConfig>, String> {
    let Some(GameTable(fields)) = game else {
        return Ok(None);
    };
    let scenario = fields
        .scenario
        .ok_or_else(|| format!("{}: [game]: scenario is required", path.display()))?;
    Ok(Some(GameConfig {
        scenario: base.join(scenario),
        saves: base.join(fields.saves.unwrap_or_else(|| PathBuf::from(DEFAULT_SAVES))),
    }))
}

fn parse_map(
    path: &Path,
    base: &Path,
    map: Option<MapTable>,
    game: Option<&GameConfig>,
) -> Result<Option<MapConfig>, String> {
    // With `[game] scenario` the map is the scenario's. `[map]` may still set the camera rates, but not a second map source.
    let (fields, directory) = match (map, game) {
        (None, None) => return Ok(None),
        (Some(MapTable(fields)), None) => {
            let directory = fields
                .directory
                .clone()
                .ok_or_else(|| format!("{}: [map]: directory is required", path.display()))?;
            (fields, base.join(directory))
        }
        (map, Some(game)) => {
            let fields = map.map(|MapTable(fields)| fields).unwrap_or_default();
            if fields.directory.is_some() {
                return Err(format!(
                    "{}: [game] scenario and [map] directory cannot both be set",
                    path.display()
                ));
            }
            (fields, game.scenario.clone())
        }
    };
    let pan = fields.pan_pixels_per_frame.unwrap_or(4);
    if !(1..=32).contains(&pan) {
        return Err(format!(
            "{}: [map]: pan_pixels_per_frame={pan} is outside 1..=32",
            path.display()
        ));
    }
    let zoom = fields.zoom_steps_per_second.unwrap_or(20);
    if !(1..=60).contains(&zoom) {
        return Err(format!(
            "{}: [map]: zoom_steps_per_second={zoom} is outside 1..=60",
            path.display()
        ));
    }
    let scroll = fields.scroll_points_per_step.unwrap_or(24);
    if !(1..=256).contains(&scroll) {
        return Err(format!(
            "{}: [map]: scroll_points_per_step={scroll} is outside 1..=256",
            path.display()
        ));
    }
    Ok(Some(MapConfig {
        directory,
        pan_pixels_per_frame: u32::try_from(pan).expect("validated map pan range fits u32"),
        zoom_steps_per_second: u32::try_from(zoom).expect("validated map zoom range fits u32"),
        scroll_points_per_step: u32::try_from(scroll)
            .expect("validated scroll point range fits u32"),
    }))
}

/// Loads the optional, tolerant player settings file without changing `Config::whole` or the strict config parser.
fn load_settings(config_path: &Path, config: &mut Config) {
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    let saves = config
        .game
        .as_ref()
        .map_or_else(|| base.join(DEFAULT_SAVES), |game| game.saves.clone());
    let path = saves.parent().unwrap_or(base).join("settings.toml");
    config.settings_file = Some(path.clone());
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            config.settings_warnings.push(format!(
                "{}: cannot read settings file: {error}; ignoring it",
                path.display()
            ));
            return;
        }
    };
    let value = match toml::from_str::<toml::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            config.settings_warnings.push(format!(
                "{}: invalid settings TOML ({error}); ignoring it",
                path.display()
            ));
            return;
        }
    };
    let Some(root) = value.as_table() else {
        config.settings_warnings.push(format!(
            "{}: settings file must contain a table; ignoring it",
            path.display()
        ));
        return;
    };
    for (table, contents) in root {
        match table.as_str() {
            "ui" => load_tooltip_settings(&path, contents, config),
            "map" => load_map_settings(&path, contents, config),
            _ => config.settings_warnings.push(format!(
                "{}: unknown settings table [{table}]; ignoring it",
                path.display()
            )),
        }
    }
    if let Some(map) = &mut config.map {
        map.pan_pixels_per_frame = config.map_settings.pan_pixels_per_frame;
        map.zoom_steps_per_second = config.map_settings.zoom_steps_per_second;
        map.scroll_points_per_step = config.map_settings.scroll_points_per_step;
    }
}

fn load_tooltip_settings(path: &Path, value: &toml::Value, config: &mut Config) {
    let Some(ui) = value.as_table() else {
        config.settings_warnings.push(format!(
            "{}: [ui] must be a table; ignoring it",
            path.display()
        ));
        return;
    };
    for (key, value) in ui {
        if key != "tooltips" {
            config.settings_warnings.push(format!(
                "{}: unknown settings table [ui.{key}]; ignoring it",
                path.display()
            ));
            continue;
        }
        let Some(entries) = value.as_table() else {
            config.settings_warnings.push(format!(
                "{}: [ui.tooltips] must be a table; ignoring it",
                path.display()
            ));
            continue;
        };
        for (key, entry) in entries {
            let mut singleton = toml::Table::new();
            singleton.insert(key.clone(), entry.clone());
            let singleton = toml::Value::Table(singleton);
            match parse_tooltips(path, Some(&singleton)) {
                Ok(validated) => match key.as_str() {
                    "delay" => {
                        config.tooltips.delay = validated.delay;
                        config.settings_overrides.tooltip_delay = Some(validated.delay);
                    }
                    "lock" => {
                        config.tooltips.lock = validated.lock;
                        config.settings_overrides.tooltip_lock = Some(validated.lock);
                    }
                    "lock_timer" => {
                        config.tooltips.lock_timer = validated.lock_timer;
                        config.settings_overrides.tooltip_lock_timer = Some(validated.lock_timer);
                    }
                    _ => config.settings_warnings.push(format!(
                        "{}: [ui.tooltips] unknown key `{key}`; accepted keys are delay, lock and lock_timer",
                        path.display()
                    )),
                },
                Err(message) => config.settings_warnings.push(message),
            }
        }
    }
}

fn load_map_settings(path: &Path, value: &toml::Value, config: &mut Config) {
    let Some(entries) = value.as_table() else {
        config.settings_warnings.push(format!(
            "{}: [map] must be a table; ignoring it",
            path.display()
        ));
        return;
    };
    for (key, entry) in entries {
        let parsed = match key.as_str() {
            "pan_pixels_per_frame" => setting_integer(path, key, entry, 1, 32),
            "zoom_steps_per_second" => setting_integer(path, key, entry, 1, 60),
            "scroll_points_per_step" => setting_integer(path, key, entry, 1, 256),
            _ => Err(format!(
                "{}: [map] {key} is not an allowed settings key; accepted keys are pan_pixels_per_frame, zoom_steps_per_second and scroll_points_per_step",
                path.display()
            )),
        };
        match parsed {
            Ok(number) => match key.as_str() {
                "pan_pixels_per_frame" => {
                    config.map_settings.pan_pixels_per_frame = number;
                    config.settings_overrides.map_pan_pixels_per_frame = Some(number);
                }
                "zoom_steps_per_second" => {
                    config.map_settings.zoom_steps_per_second = number;
                    config.settings_overrides.map_zoom_steps_per_second = Some(number);
                }
                "scroll_points_per_step" => {
                    config.map_settings.scroll_points_per_step = number;
                    config.settings_overrides.map_scroll_points_per_step = Some(number);
                }
                _ => unreachable!("unknown settings keys are errors"),
            },
            Err(message) => config.settings_warnings.push(message),
        }
    }
}

fn setting_integer(
    path: &Path,
    key: &str,
    value: &toml::Value,
    minimum: u32,
    maximum: u32,
) -> Result<u32, String> {
    value
        .as_integer()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| (*number >= minimum) && (*number <= maximum))
        .ok_or_else(|| {
            format!(
                "{}: [map] {key} = {value} is not accepted: use an integer in {minimum}..={maximum}",
                path.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/tmp/codekarl-test/config.toml";

    fn input_action<'a>(
        snapshot: &'a ck_input::InputSnapshot,
        name: &str,
    ) -> &'a ck_input::ActionSnapshot {
        snapshot
            .actions
            .iter()
            .find(|state| state.name == name)
            .unwrap_or_else(|| panic!("missing action {name}"))
    }

    fn parse_config(text: &str) -> Result<Config, String> {
        parse(Path::new(PATH), text)
    }

    fn parse_text(text: &str) -> Result<LogConfig, String> {
        parse_config(text).map(|config| config.log)
    }

    #[test]
    fn the_defaults_equal_an_empty_file_and_an_empty_log_table() {
        let defaults = LogConfig::default();
        assert_eq!(defaults.level, Level::Info);
        assert!(defaults.timestamps);
        assert_eq!(defaults.colour, ColourMode::Auto);
        assert!(defaults.categories.is_empty());
        assert_eq!(parse_text(""), Ok(LogConfig::default()));
        assert_eq!(parse_text("[log]\n"), Ok(LogConfig::default()));
        assert_eq!(parse_text("# only a comment\n"), Ok(LogConfig::default()));
    }

    #[test]
    fn shipped_game_config_loads_existing_game_inputs_and_selects_the_default_binary() {
        let app = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = app.join("../..");
        let config_path = root.join("config/game.toml");
        let config =
            load_without_settings(&config_path).expect("the shipped game configuration loads");

        let assets = config.render.assets.expect("render assets are configured");
        assert!(
            assets.is_dir(),
            "missing assets directory: {}",
            assets.display()
        );
        let game = config.game.expect("the game is configured");
        assert!(
            game.scenario.is_dir(),
            "missing scenario: {}",
            game.scenario.display()
        );
        assert_eq!(game.saves, config_path.parent().unwrap().join("../saves"));
        for name in ["ui", "gameplay"] {
            let vm = config
                .vms
                .iter()
                .find(|vm| vm.name == name)
                .expect("configured VM");
            assert!(
                vm.entry.is_file(),
                "missing {} entry: {}",
                name,
                vm.entry.display()
            );
        }

        let manifest: toml::Value =
            toml::from_str(&fs::read_to_string(app.join("Cargo.toml")).expect("read app manifest"))
                .expect("the app manifest is valid TOML");
        assert_eq!(
            manifest["package"]["default-run"].as_str(),
            Some("codekarl")
        );
    }

    #[test]
    fn settings_file_overrides_the_tooltip_values_without_changing_config() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, "[ui.tooltips]\ndelay = 0.4\nlock = \"timer\"\n").expect("write config");
        fs::write(
            dir.0.join("settings.toml"),
            "[ui.tooltips]\ndelay = 0.8\nlock = \"disabled\"\n",
        )
        .expect("write settings");

        let config = load(&path).expect("settings overrides are optional and valid");

        assert!((config.tooltips.delay - 0.8).abs() < f64::EPSILON);
        assert_eq!(config.tooltips.lock, "disabled");
        assert_eq!(
            config.whole,
            to_config_value(
                &toml::from_str::<toml::Value>("[ui.tooltips]\ndelay = 0.4\nlock = \"timer\"\n")
                    .unwrap()
            )
        );
    }

    #[test]
    fn settings_file_keeps_valid_siblings_and_reports_invalid_entries_by_name() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("config.toml");
        fs::write(
            &path,
            "[map]\ndirectory = 'map'\nzoom_steps_per_second = 7\n",
        )
        .expect("write config");
        fs::write(
            dir.0.join("settings.toml"),
            "[ui.tooltips]\ndelay = 0.8\nlock_timer = 0.25\nextra = true\n[map]\npan_pixels_per_frame = 9\ndirectory = 'wrong'\nscroll_points_per_step = 400\n",
        )
        .expect("write settings");

        let config = load(&path).expect("invalid settings entries are ignored");

        assert!((config.tooltips.delay - 0.8).abs() < f64::EPSILON);
        assert!((config.tooltips.lock_timer - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.map_settings.pan_pixels_per_frame, 9);
        assert_eq!(config.map_settings.zoom_steps_per_second, 7);
        assert_eq!(config.map_settings.scroll_points_per_step, 24);
        assert_eq!(config.map.as_ref().unwrap().pan_pixels_per_frame, 9);
        assert_eq!(config.settings_warnings.len(), 4);
        for name in ["lock_timer", "extra", "directory", "scroll_points_per_step"] {
            if name == "scroll_points_per_step" {
                assert!(
                    config
                        .settings_warnings
                        .iter()
                        .any(|warning| warning.contains(name))
                );
            } else if name == "directory" {
                assert!(
                    config
                        .settings_warnings
                        .iter()
                        .any(|warning| warning.contains("[map] directory"))
                );
            } else {
                assert!(
                    config
                        .settings_warnings
                        .iter()
                        .any(|warning| warning.contains(name))
                );
            }
        }
    }

    #[test]
    fn malformed_settings_are_ignored_and_missing_settings_are_silent() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, "[ui.tooltips]\ndelay = 0.4\n").expect("write config");
        let absent = load(&path).expect("missing settings are allowed");
        assert!(absent.settings_warnings.is_empty());
        assert!((absent.tooltips.delay - 0.4).abs() < f64::EPSILON);

        fs::write(dir.0.join("settings.toml"), "[ui.tooltips\ndelay = 1")
            .expect("write invalid TOML");
        let malformed = load(&path).expect("malformed settings are ignored");
        assert!((malformed.tooltips.delay - 0.4).abs() < f64::EPSILON);
        assert_eq!(malformed.settings_warnings.len(), 1);
        assert!(malformed.settings_warnings[0].contains("settings.toml"));
    }

    #[test]
    fn settings_path_uses_the_parent_of_the_configured_saves_directory() {
        let dir = crate::testing::Dir::new();
        let config_dir = dir.0.join("config");
        fs::create_dir_all(&config_dir).expect("create config directory");
        let path = config_dir.join("game.toml");
        fs::write(
            &path,
            "[game]\nscenario = '../scenario'\nsaves = '../saves'\n",
        )
        .expect("write config");
        fs::write(
            dir.0.join("settings.toml"),
            "[map]\nzoom_steps_per_second = 33\n",
        )
        .expect("write settings beside the saves directory");

        let config = load(&path).expect("load configured saves and settings");

        assert_eq!(
            config.settings_file,
            Some(config_dir.join("../settings.toml"))
        );
        assert_eq!(config.map_settings.zoom_steps_per_second, 33);
    }

    #[test]
    fn every_bad_settings_entry_is_ignored_independently() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, "").expect("write config");
        fs::write(
            dir.0.join("settings.toml"),
            "[ui.tooltips]\ndelay = 0.25\nlock = 'never'\nlock_timer = 3.1\nextra = true\n\n[map]\npan_pixels_per_frame = 0\nzoom_steps_per_second = 61\nscroll_points_per_step = '24'\ndirectory = 'map'\n\n[graphics]\nscale = 2\n",
        )
        .expect("write bad values");

        let config = load(&path).expect("bad settings do not prevent startup");

        assert_eq!(config.tooltips, TooltipsConfig::default());
        assert_eq!(config.map_settings, MapSettings::default());
        assert_eq!(config.settings_warnings.len(), 9);
        for name in [
            "delay",
            "lock",
            "lock_timer",
            "extra",
            "pan_pixels_per_frame",
            "zoom_steps_per_second",
            "scroll_points_per_step",
            "directory",
            "graphics",
        ] {
            assert!(
                config
                    .settings_warnings
                    .iter()
                    .any(|warning| warning.contains(name)),
                "missing warning for {name}: {:?}",
                config.settings_warnings
            );
        }
    }

    #[test]
    fn an_out_of_range_tooltip_delay_is_ignored_without_losing_valid_siblings() {
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, "").expect("write config");
        fs::write(
            dir.0.join("settings.toml"),
            "[ui.tooltips]\ndelay = 2.1\nlock_timer = 2.0\n",
        )
        .expect("write settings");

        let config = load(&path).expect("invalid delay is ignored");

        assert!((config.tooltips.delay - TooltipsConfig::default().delay).abs() < 1.0e-9);
        assert!((config.tooltips.lock_timer - 2.0).abs() < 1.0e-9);
        assert_eq!(config.settings_warnings.len(), 1);
        assert!(config.settings_warnings[0].contains("delay"));
        assert!(config.settings_warnings[0].contains("2.1"));
    }

    #[test]
    fn resource_and_scene_settings_fill_the_spec_defaults() {
        let config = parse_config("").expect("built-in defaults");
        assert_eq!(config.resources, ResourceConfig::default());
        assert_eq!(config.render.max_quads, 131_072);
        // The application's default texture count is its own (CK-201); every other service limit is the service's default.
        assert_eq!(
            config.resources.service,
            ResourceLimits {
                max_textures: DEFAULT_MAX_TEXTURES,
                ..ResourceLimits::DEFAULT
            }
        );
        let configured = parse_config(
            "[resources]\nmax_textures=7\nmax_pending_jobs=8\ndecode_workers=2\n\
             cpu_pixel_bytes=67108864\nmetadata_bytes=1048576\nmax_prepared_images=32\n\
             max_upload_images_per_batch=16\nmax_upload_bytes_per_batch=1048576\n\
             staging_bytes=134217728\ntexture_bytes=67108864\nmax_extra_revisions=3\n\
             max_adoptions_per_frame=4\nbinding_geometry_bytes=1048576\n\
             [render]\nmax_quads=99\n",
        )
        .expect("all settings are read");
        assert_eq!(configured.resources.service.max_textures, 7);
        assert_eq!(configured.resources.service.max_pending_jobs, 8);
        assert_eq!(configured.resources.service.decode_workers, 2);
        assert_eq!(configured.resources.service.cpu_pixel_bytes, 64 << 20);
        assert_eq!(configured.resources.service.metadata_bytes, 1 << 20);
        assert_eq!(configured.resources.max_prepared_images, 32);
        assert_eq!(configured.resources.max_upload_images_per_batch, 16);
        assert_eq!(configured.resources.max_upload_bytes_per_batch, 1 << 20);
        assert_eq!(configured.resources.staging_bytes, 128 << 20);
        assert_eq!(configured.resources.texture_bytes, 64 << 20);
        assert_eq!(configured.resources.max_extra_revisions, 3);
        assert_eq!(configured.resources.max_adoptions_per_frame, 4);
        assert_eq!(configured.resources.binding_geometry_bytes, 1 << 20);
        assert_eq!(configured.render.max_quads, 99);
        let ConfigValue::Table(whole) = configured.whole else {
            panic!("whole table");
        };
        let ConfigValue::Table(resources) = &whole["resources"] else {
            panic!("VM resources table");
        };
        let ConfigValue::Table(render) = &whole["render"] else {
            panic!("VM render table");
        };
        assert_eq!(resources["max_textures"], ConfigValue::Integer(7));
        assert_eq!(resources["decode_workers"], ConfigValue::Integer(2));
        assert_eq!(
            resources["max_adoptions_per_frame"],
            ConfigValue::Integer(4)
        );
        assert_eq!(render["max_quads"], ConfigValue::Integer(99));
    }

    #[test]
    fn map_is_opt_in_with_mockup_derived_keyboard_defaults_and_custom_rates() {
        let without_map = parse_config("").expect("empty configuration");
        assert!(without_map.map.is_none());
        assert_eq!(
            without_map.bindings,
            ck_window::LoopSettings::default().bindings().clone()
        );

        let defaults = parse_config("[map]\ndirectory = \"fixture\"\n").expect("map defaults");
        let map = defaults.map.expect("configured map");
        assert_eq!(
            map.directory,
            Path::new(PATH).parent().unwrap().join("fixture")
        );
        assert_eq!(map.pan_pixels_per_frame, 4);
        assert_eq!(map.zoom_steps_per_second, 20);
        assert_eq!(map.scroll_points_per_step, 24);
        assert_eq!(
            defaults.bindings.action("map_pan_left").unwrap().bindings()[0]
                .key()
                .name(),
            "ArrowLeft"
        );
        let zoom_in = defaults.bindings.action("map_zoom_in").unwrap().bindings();
        assert_eq!(zoom_in.len(), 2);
        assert_eq!(zoom_in[0].key().name(), "Equal");
        assert_eq!(zoom_in[1].key().name(), "Equal");
        let expected_zoom_in = Bindings::from_entries([(
            "map_zoom_in".to_owned(),
            vec!["Equal".to_owned(), "Shift+Equal".to_owned()],
        )])
        .expect("valid map zoom defaults");
        assert_eq!(
            zoom_in,
            expected_zoom_in
                .action("map_zoom_in")
                .expect("zoom action")
                .bindings()
        );
        assert_eq!(
            defaults.bindings.action("map_zoom_out").unwrap().bindings()[0]
                .key()
                .name(),
            "Minus"
        );

        let mut mapper = ck_input::Mapper::new(defaults.bindings.clone());
        mapper.key(ck_input::Key::Equal, true, false);
        let plain_equal = mapper.snapshot();
        assert!(input_action(&plain_equal, "map_zoom_in").pressed);
        mapper.acknowledge(plain_equal.sequence);
        mapper.key(ck_input::Key::Equal, false, false);
        let released_equal = mapper.snapshot();
        mapper.acknowledge(released_equal.sequence);
        mapper.key(ck_input::Key::ShiftLeft, true, false);
        mapper.key(ck_input::Key::Equal, true, false);
        let shifted_equal = mapper.snapshot();
        assert!(input_action(&shifted_equal, "map_zoom_in").pressed);
        assert!(!input_action(&shifted_equal, "map_zoom_out").pressed);
        mapper.acknowledge(shifted_equal.sequence);
        mapper.key(ck_input::Key::Minus, true, false);
        let modified_minus = mapper.snapshot();
        assert!(!input_action(&modified_minus, "map_zoom_out").held);
        assert!(!modified_minus.actions.iter().any(|state| state.pressed));
        mapper.key(ck_input::Key::ArrowLeft, true, false);
        let modified_arrow = mapper.snapshot();
        assert!(!input_action(&modified_arrow, "map_pan_left").held);

        let custom = parse_config(
            "[map]\ndirectory = \"fixture\"\npan_pixels_per_frame = 9\nzoom_steps_per_second = 31\nscroll_points_per_step = 40\n\
             [input.bindings]\nmap_pan_left = [\"KeyA\"]\n",
        )
        .expect("custom map controls");
        let map = custom.map.expect("configured map");
        assert_eq!(map.pan_pixels_per_frame, 9);
        assert_eq!(map.zoom_steps_per_second, 31);
        assert_eq!(map.scroll_points_per_step, 40);
        assert_eq!(
            custom.bindings.action("map_pan_left").unwrap().bindings()[0]
                .key()
                .name(),
            "KeyA"
        );
        assert_eq!(
            custom.bindings.action("map_pan_right").unwrap().bindings()[0]
                .key()
                .name(),
            "ArrowRight"
        );
    }

    #[test]
    fn map_control_rates_reject_values_outside_their_approved_ranges() {
        for (key, value, range) in [
            ("pan_pixels_per_frame", "0", "1..=32"),
            ("pan_pixels_per_frame", "33", "1..=32"),
            ("zoom_steps_per_second", "0", "1..=60"),
            ("zoom_steps_per_second", "61", "1..=60"),
            ("scroll_points_per_step", "0", "1..=256"),
            ("scroll_points_per_step", "257", "1..=256"),
        ] {
            let text = format!("[map]\ndirectory = \"fixture\"\n{key} = {value}\n");
            let error = parse_config(&text).expect_err(&text);
            assert!(error.contains(key) && error.contains(range), "{error}");
        }
    }

    #[test]
    fn every_new_setting_rejects_one_past_its_range_and_names_the_key() {
        let cases = [
            ("max_textures", "0", "1..=1048576"),
            ("max_pending_jobs", "1048577", "1..=1048576"),
            ("decode_workers", "17", "1..=16"),
            ("cpu_pixel_bytes", "67108863", "67108864..=68719476736"),
            ("metadata_bytes", "17179869185", "1048576..=17179869184"),
            ("max_prepared_images", "4097", "2..=4096"),
            ("max_upload_images_per_batch", "2049", "1..=2048"),
            (
                "max_upload_bytes_per_batch",
                "1048575",
                "1048576..=268435456",
            ),
            ("staging_bytes", "134217727", "134217728..=4294967296"),
            ("texture_bytes", "67108863", "67108864..=68719476736"),
            ("max_extra_revisions", "0", "1..=65536"),
            ("max_adoptions_per_frame", "65537", "1..=65536"),
            (
                "binding_geometry_bytes",
                "4294967297",
                "1048576..=4294967296",
            ),
        ];
        for (key, value, range) in cases {
            let text = format!("[resources]\n{key}={value}\n");
            let error = parse_config(&text).expect_err(&text);
            assert!(
                error.contains(key) && error.contains(range),
                "{key}: {error}"
            );
        }
        for value in ["0", "1048577"] {
            let text = format!("[render]\nmax_quads={value}\n");
            let error = parse_config(&text).expect_err(&text);
            assert!(
                error.contains("max_quads") && error.contains("1..=1048576"),
                "{error}"
            );
        }
        for value in ["0", "65537"] {
            let text = format!("[render]\nmax_groups={value}\n");
            let error = parse_config(&text).expect_err(&text);
            assert!(
                error.contains("max_groups") && error.contains("1..=65536"),
                "{error}"
            );
        }
    }

    #[test]
    fn every_new_setting_accepts_both_range_endpoints() {
        let cases = [
            // The top of `max_textures` is bounded by the bindless table (design §7.1): with the default `max_extra_revisions` (1024)
            // and the checker's slot the largest table (1,000,000 slots) covers 998,975 textures; 1,048,576 is refused with the
            // three numbers (`the_bindless_table_must_cover_the_textures_the_extra_revisions_and_the_checker`).
            ("max_textures", "1", "1", ""),
            ("max_pending_jobs", "1", "1048576", ""),
            ("decode_workers", "1", "16", ""),
            ("cpu_pixel_bytes", "67108864", "68719476736", ""),
            ("metadata_bytes", "1048576", "17179869184", ""),
            (
                "max_prepared_images",
                "2",
                "4096",
                "max_upload_images_per_batch=1\n",
            ),
            (
                "max_upload_images_per_batch",
                "1",
                "2048",
                "max_prepared_images=4096\n",
            ),
            (
                "max_upload_bytes_per_batch",
                "1048576",
                "268435456",
                "staging_bytes=536870912\n",
            ),
            ("staging_bytes", "134217728", "4294967296", ""),
            ("texture_bytes", "67108864", "68719476736", ""),
            // 65,536 textures + 65,536 extra revisions + the checker is one above the default table: a larger one is named.
            (
                "max_extra_revisions",
                "1",
                "65536",
                "[render]\nbindless_texture_slots=200000\n",
            ),
            ("max_adoptions_per_frame", "1", "65536", ""),
            ("binding_geometry_bytes", "1048576", "4294967296", ""),
        ];
        for (key, minimum, maximum, companion) in cases {
            for value in [minimum, maximum] {
                let text = format!("[resources]\n{key}={value}\n{companion}");
                parse_config(&text).unwrap_or_else(|error| {
                    panic!("{key}={value} endpoint must be accepted: {error}")
                });
            }
        }
        // The top `max_textures` that any table can cover, with the largest table.
        parse_config(
            "[resources]\nmax_textures=998975\n[render]\nbindless_texture_slots=1000000\n",
        )
        .expect("the largest table covers 998,975 textures and 1,024 extra revisions");
        for value in ["2", "1000000"] {
            let text = format!("[render]\nbindless_texture_slots={value}\n");
            let result = parse_config(&text);
            // The largest table accepts the default capacities; the smallest table is below the rule and refused (below).
            assert_eq!(result.is_ok(), value == "1000000", "{value}: {result:?}");
        }
        for value in ["1", "1048576"] {
            let text = format!("[render]\nmax_quads={value}\n");
            parse_config(&text).unwrap_or_else(|error| {
                panic!("render.max_quads={value} endpoint must be accepted: {error}")
            });
        }
        for value in ["1", "4096", "65536"] {
            let text = format!("[render]\nmax_groups={value}\n");
            let config = parse_config(&text).unwrap_or_else(|error| {
                panic!("render.max_groups={value} endpoint must be accepted: {error}")
            });
            assert_eq!(config.render.max_groups.to_string(), value);
        }
        assert_eq!(parse_config("").unwrap().render.max_groups, 4_096);
    }

    #[test]
    fn the_bindless_table_must_cover_the_textures_the_extra_revisions_and_the_checker() {
        // Default: 4,096 slots against 2,048 + 1,024 + 1 (CK-201: the table's size is a per-draw cost on MoltenVK).
        let defaults = parse_config("").expect("defaults");
        assert_eq!(defaults.render.bindless_texture_slots, 4_096);
        assert_eq!(defaults.resources.service.max_textures, 2_048);
        assert_eq!(defaults.resources.max_extra_revisions, 1_024);
        assert!(
            u64::from(defaults.render.bindless_texture_slots)
                > u64::from(defaults.resources.service.max_textures)
                    + defaults.resources.max_extra_revisions as u64
        );
        // A large table still works when it is configured with the limits it covers: the scale runs' values.
        let scale = "[resources]\nmax_textures=65536\nmax_extra_revisions=1024\n[render]\nbindless_texture_slots=131072\n";
        let scaled = parse_config(scale).expect("the scale runs' explicit limits");
        assert_eq!(scaled.render.bindless_texture_slots, 131_072);
        assert_eq!(scaled.resources.service.max_textures, 65_536);
        // Asking for more textures than the default table covers is refused with the three numbers, never accepted silently.
        let over = "[resources]\nmax_textures=4096\n";
        let error = parse_config(over).expect_err("more textures than the default table covers");
        assert!(
            error.contains("bindless_texture_slots=4096")
                && error.contains("max_textures=4096")
                && error.contains("max_extra_revisions=1024")
                && error.contains("5121"),
            "{error}"
        );
        // One below the rule is refused with the three numbers; exactly the rule is accepted (negative control: a check that
        // forgot the checker's slot would accept 67,560 here).
        let below = "[resources]\nmax_textures=65536\nmax_extra_revisions=1024\n[render]\nbindless_texture_slots=66560\n";
        let error = parse_config(below).expect_err("one below the minimum");
        assert!(
            error.contains("bindless_texture_slots=66560")
                && error.contains("max_textures=65536")
                && error.contains("max_extra_revisions=1024")
                && error.contains("66561"),
            "{error}"
        );
        let exact = below.replace("66560", "66561");
        assert_eq!(
            parse_config(&exact)
                .expect("exactly the rule")
                .render
                .bindless_texture_slots,
            66_561
        );
        // The `[resources]` values move the same slot count from accepted to refused, and back.
        let moved = "[resources]\nmax_textures=65537\nmax_extra_revisions=1024\n[render]\nbindless_texture_slots=66561\n";
        assert!(
            parse_config(moved)
                .expect_err("one more texture")
                .contains("66562")
        );
        let fewer = "[resources]\nmax_textures=100\nmax_extra_revisions=10\n[render]\nbindless_texture_slots=111\n";
        assert!(parse_config(fewer).is_ok());
        // Out of the table's range: refused before the rule is looked at.
        for value in ["0", "1", "1000001", "4294967296"] {
            let text = format!("[render]\nbindless_texture_slots={value}\n");
            let error = parse_config(&text).expect_err("outside the range");
            assert!(error.contains("2..=1000000"), "{value}: {error}");
        }
        // The top of the `max_textures` range cannot be covered by any table: refused with the three numbers.
        let top = "[resources]\nmax_textures=1048576\n[render]\nbindless_texture_slots=1000000\n";
        let error = parse_config(top).expect_err("more textures than any table holds");
        assert!(
            error.contains("max_textures=1048576") && error.contains("1049601"),
            "{error}"
        );
    }

    #[test]
    fn a_capacity_sum_that_would_overflow_u32_is_refused_not_wrapped() {
        // The sum is taken with `checked_add`: `max_textures + max_extra_revisions + 1` above `u32::MAX` is its own refusal. (The
        // `[resources]` ranges keep a parsed pair far below it, so the helper is exercised with the values the ranges forbid.)
        let resources = ResourceConfig {
            max_extra_revisions: usize::try_from(u32::MAX).expect("64-bit"),
            ..ResourceConfig::default()
        };
        let error = bindless_slots(Path::new(PATH), Some(1_000_000), &resources)
            .expect_err("the sum overflows u32");
        assert!(error.contains("overflows u32"), "{error}");
        let wrapped = ResourceConfig {
            max_extra_revisions: usize::try_from(u32::MAX).expect("64-bit")
                - usize::try_from(DEFAULT_MAX_TEXTURES).expect("64-bit"),
            ..ResourceConfig::default()
        };
        // max_textures + (u32::MAX - max_textures) + 1 wraps to 0 in u32 arithmetic: a wrapping sum would accept any table.
        let error =
            bindless_slots(Path::new(PATH), Some(2), &wrapped).expect_err("wrapped to zero");
        assert!(error.contains("overflows u32"), "{error}");
    }

    #[test]
    fn resource_tables_reject_unknown_and_cross_inconsistent_limits() {
        let unknown = parse_config("[resources]\nmax_texture=7\n").expect_err("unknown key");
        assert!(unknown.contains("max_texture"), "{unknown}");
        let prepared =
            parse_config("[resources]\nmax_prepared_images=2\n").expect_err("inconsistent caps");
        assert!(
            prepared.contains("max_prepared_images") && prepared.contains("twice"),
            "{prepared}"
        );
        let bytes = parse_config("[resources]\nmax_upload_bytes_per_batch=268435456\n")
            .expect_err("staging cap");
        assert!(
            bytes.contains("max_upload_bytes_per_batch") && bytes.contains("staging_bytes/2"),
            "{bytes}"
        );
    }

    #[test]
    fn every_key_is_read() {
        let config = parse_text(
            "[log]\nlevel = \"warn\"\ntimestamps = false\ncolour = \"always\"\n\
             [log.categories]\nruntime = \"debug\"\n\"lua.ui\" = \"trace\"\n",
        )
        .expect("a valid file");
        assert_eq!(config.level, Level::Warn);
        assert!(!config.timestamps);
        assert_eq!(config.colour, ColourMode::Always);
        assert_eq!(
            config.categories,
            [
                ("lua.ui".to_owned(), Level::Trace),
                ("runtime".to_owned(), Level::Debug)
            ]
        );
    }

    #[test]
    fn the_settings_follow_the_category_rules() {
        let config = parse_text(
            "[log]\nlevel = \"warn\"\n[log.categories]\nruntime = \"info\"\n\"lua.ui\" = \"debug\"\n",
        )
        .expect("a valid file");
        let settings = config.settings(false, None).expect("valid categories");
        assert!(settings.filter.allows(Level::Info, "runtime"));
        assert!(!settings.filter.allows(Level::Debug, "runtime"));
        assert!(settings.filter.allows(Level::Debug, "lua.ui.deep"));
        assert!(!settings.filter.allows(Level::Info, "config"));
        assert!(settings.filter.allows(Level::Warn, "config"));
        assert!(settings.timestamps);
        assert!(!settings.colour, "auto colour is off for a pipe");
    }

    #[test]
    fn auto_colour_needs_a_terminal_and_no_no_color() {
        let config = LogConfig::default();
        assert!(config.settings(true, None).expect("settings").colour);
        assert!(
            !config
                .settings(true, Some(OsStr::new("1")))
                .expect("settings")
                .colour
        );
        assert!(!config.settings(false, None).expect("settings").colour);
        let always = parse_text("[log]\ncolour = \"always\"\n").expect("valid");
        assert!(
            always
                .settings(false, Some(OsStr::new("1")))
                .expect("settings")
                .colour
        );
        let never = parse_text("[log]\ncolour = \"never\"\n").expect("valid");
        assert!(!never.settings(true, None).expect("settings").colour);
    }

    #[test]
    fn invalid_toml_names_the_path_and_the_line() {
        let error = parse_text("[log]\nlevel = = 3\n").expect_err("invalid TOML");
        assert!(error.starts_with(PATH), "{error}");
        assert!(error.contains("line 2"), "{error}");
        assert!(!error.contains('\n'), "{error}");
        let first = parse_text("[log\n").expect_err("invalid TOML");
        assert!(first.contains("line 1"), "{first}");
    }

    #[test]
    fn unknown_keys_are_rejected_at_every_level() {
        let top = parse_text("\n\nverbose = true\n").expect_err("unknown key");
        assert!(top.contains("line 3") && top.contains("verbose"), "{top}");
        let log = parse_text("[log]\nlevel = \"info\"\nlouder = true\n").expect_err("unknown key");
        assert!(log.contains("line 3") && log.contains("louder"), "{log}");
        let table = parse_text("[logging]\n").expect_err("unknown table");
        assert!(table.contains("logging"), "{table}");
    }

    #[test]
    fn wrong_types_and_invalid_names_name_the_value() {
        let level = parse_text("[log]\nlevel = \"loud\"\n").expect_err("invalid level");
        assert!(
            level.starts_with(PATH) && level.contains("line 2"),
            "{level}"
        );
        assert!(level.contains("loud"), "{level}");
        let colour = parse_text("[log]\ncolour = \"bright\"\n").expect_err("invalid colour");
        assert!(colour.contains("bright"), "{colour}");
        let kind = parse_text("[log]\ntimestamps = \"yes\"\n").expect_err("wrong type");
        assert!(kind.contains("line 2"), "{kind}");
        let number = parse_text("[log]\nlevel = 3\n").expect_err("wrong type");
        assert!(number.contains("line 2"), "{number}");
        let category = parse_text("[log.categories]\n\"bad name\" = \"info\"\n")
            .expect_err("invalid category name");
        assert!(category.contains("bad name"), "{category}");
        let category_level =
            parse_text("[log.categories]\nruntime = \"loud\"\n").expect_err("invalid level");
        assert!(category_level.contains("loud"), "{category_level}");
    }

    #[test]
    fn log_must_be_a_table_not_an_array() {
        for text in [
            "log = []\n",
            "log = [\"warn\"]\n",
            "log = [\"info\", false, \"never\"]\n",
            "\nlog = [{ level = \"warn\" }]\n",
        ] {
            let error = parse_text(text).expect_err("an array is not a table");
            assert!(error.starts_with(PATH), "{text:?}: {error}");
            assert!(
                error.contains("`log`"),
                "{text:?} does not name the key: {error}"
            );
            assert!(error.contains("line "), "{text:?}: {error}");
            assert!(!error.contains('\n'), "{text:?}: {error}");
        }
        // Wrong scalar types for the whole table, and an array for the categories, are rejected too.
        for text in ["log = 3\n", "log = \"warn\"\n", "[log]\ncategories = []\n"] {
            assert!(parse_text(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_table_is_accepted_in_every_toml_spelling() {
        let expected = parse_text("[log]\nlevel = \"warn\"\ntimestamps = false\n").expect("valid");
        assert_eq!(expected.level, Level::Warn);
        assert_eq!(
            parse_text("log = { level = \"warn\", timestamps = false }\n"),
            Ok(expected),
            "an inline table"
        );
        assert_eq!(parse_text("[log]\n"), Ok(LogConfig::default()));
        assert_eq!(parse_text("log = {}\n"), Ok(LogConfig::default()));
        assert_eq!(
            parse_text(""),
            Ok(LogConfig::default()),
            "absent means the defaults"
        );
    }

    #[test]
    fn an_unreadable_file_names_the_path_and_the_os_error() {
        let error =
            load(Path::new("/nonexistent/codekarl-test/missing.toml")).expect_err("no such file");
        assert_eq!(
            error,
            "cannot read /nonexistent/codekarl-test/missing.toml: \
             No such file or directory (os error 2)"
        );
    }

    // ---- the `runtime` table

    fn deadlines(text: &str) -> Result<OwnerDeadlines, String> {
        parse_config(text).map(|config| config.deadlines)
    }

    #[test]
    fn the_owner_deadlines_default_to_5_5_and_1_seconds() {
        let defaults = OwnerDeadlines {
            start: Duration::from_secs(5),
            stop: Duration::from_secs(5),
            cleanup: Duration::from_secs(1),
        };
        assert_eq!(OwnerDeadlines::default(), defaults);
        assert_eq!(deadlines(""), Ok(defaults));
        assert_eq!(deadlines("[runtime]\n"), Ok(defaults));
    }

    #[test]
    fn each_timeout_is_read_as_integer_or_fractional_seconds() {
        let read =
            deadlines("[runtime]\nstart_timeout = 60\nstop_timeout = 0.2\ncleanup_timeout = 2.5\n")
                .expect("valid timeouts");
        assert_eq!(read.start, Duration::from_secs(60));
        assert_eq!(read.stop, Duration::from_millis(200));
        assert_eq!(read.cleanup, Duration::from_millis(2500));
        let one = deadlines("[runtime]\nstop_timeout = 1\n").expect("one second");
        assert_eq!(one.stop, Duration::from_secs(1));
        assert_eq!(
            one.start,
            Duration::from_secs(5),
            "the others keep their defaults"
        );
        assert_eq!(
            deadlines("runtime = { start_timeout = 7 }\n")
                .expect("inline")
                .start,
            Duration::from_secs(7)
        );
    }

    #[test]
    fn timeouts_that_are_not_numbers_or_are_out_of_range_are_configuration_errors() {
        for value in [
            "\"5\"",
            "true",
            "[5]",
            "{ a = 1 }",
            "nan",
            "-nan",
            "inf",
            "-inf",
            "0",
            "0.0",
            "-1",
            "-0.5",
            "60.0001",
            "61",
            "1e300",
            "1e-400",
            "9223372036854775807",
            "-9223372036854775808",
            "2026-10-19",
            "2026-10-19T10:00:00Z",
        ] {
            for key in ["start_timeout", "stop_timeout", "cleanup_timeout"] {
                let text = format!("[runtime]\n{key} = {value}\n");
                let error = deadlines(&text).expect_err(&text);
                assert!(
                    error.starts_with(PATH) && error.contains("line 2"),
                    "{text:?}: {error}"
                );
                assert!(!error.contains('\n'), "{text:?}: {error}");
            }
        }
        let range = deadlines("[runtime]\nstop_timeout = 61\n").expect_err("out of range");
        assert!(
            range.contains("61") && range.contains("at most 60"),
            "{range}"
        );
        let kind = deadlines("[runtime]\nstop_timeout = \"5\"\n").expect_err("a string");
        assert!(kind.contains("number of seconds"), "{kind}");
    }

    #[test]
    fn frame_absent_gives_the_design_defaults() {
        for text in ["", "[frame]\n", "# nothing\n"] {
            let config = parse_config(text).expect(text);
            assert_eq!(config.frame, FrameConfig::default(), "{text:?}");
            assert_eq!(config.frame.updates_per_second, 60);
            assert_eq!(config.frame.max_delta, Duration::from_millis(100));
        }
        // Each key alone leaves the other at its default.
        let rate = parse_config("[frame]\nupdates_per_second = 30\n").expect("a rate");
        assert_eq!(rate.frame.updates_per_second, 30);
        assert_eq!(rate.frame.max_delta, Duration::from_millis(100));
        let delta = parse_config("[frame]\nmax_delta_seconds = 0.25\n").expect("a delta");
        assert_eq!(delta.frame.updates_per_second, 60);
        assert_eq!(delta.frame.max_delta, Duration::from_millis(250));
    }

    #[test]
    fn tooltip_settings_default_and_accept_endpoints_and_export_to_lua() {
        let defaults = parse_config("").expect("empty config").tooltips;
        assert_eq!(defaults, TooltipsConfig::default());
        let configured =
            parse_config("[ui.tooltips]\ndelay = 0\nlock = \"middle_mouse\"\nlock_timer = 3\n")
                .expect("valid tooltip settings")
                .tooltips;
        assert!((configured.delay - 0.0).abs() < f64::EPSILON);
        assert_eq!(configured.lock, "middle_mouse");
        assert!((configured.lock_timer - 3.0).abs() < f64::EPSILON);
        let ConfigValue::Table(table) = configured.value() else {
            panic!("tooltip settings table")
        };
        assert_eq!(table.get("delay"), Some(&ConfigValue::Float(0.0)));
        assert_eq!(
            table.get("lock"),
            Some(&ConfigValue::String("middle_mouse".to_owned()))
        );
        assert_eq!(table.get("lock_timer"), Some(&ConfigValue::Float(3.0)));
    }

    #[test]
    fn invalid_tooltip_settings_name_the_table_key_and_accepted_values() {
        for (text, key) in [
            ("[ui.tooltips]\ndelay = 2.1\n", "delay"),
            ("[ui.tooltips]\ndelay = 0.25\n", "delay"),
            ("[ui.tooltips]\nlock = \"always\"\n", "lock"),
            ("[ui.tooltips]\nlock_timer = 0.2\n", "lock_timer"),
            ("[ui.tooltips]\nother = 1\n", "other"),
            ("[ui.tooltips]\ndelay = \"0.3\"\n", "delay"),
        ] {
            let error = parse_config(text).expect_err(text);
            assert!(
                error.contains("[ui.tooltips]") && error.contains(key),
                "{text:?}: {error}"
            );
        }
    }

    #[test]
    fn frame_values_are_accepted_at_the_ends_of_their_ranges() {
        for (rate, delta, seconds) in [
            ("1", "1.0", Duration::from_secs(1)),
            ("1", "1", Duration::from_secs(1)),
            ("240", "0.001", Duration::from_millis(1)),
            ("240", "0.5", Duration::from_millis(500)),
        ] {
            let text =
                format!("[frame]\nupdates_per_second = {rate}\nmax_delta_seconds = {delta}\n");
            let config = parse_config(&text).expect(&text);
            assert_eq!(config.frame.updates_per_second.to_string(), rate, "{text}");
            assert_eq!(config.frame.max_delta, seconds, "{text}");
            config.frame.update_settings().expect("the loop takes them");
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the table of refusals and the checks on each read best together"
    )]
    fn a_frame_value_out_of_range_fails_the_load_naming_the_key_the_value_and_the_range() {
        for (text, key, shown, range) in [
            (
                "updates_per_second = 0",
                "updates_per_second",
                "0",
                "1 to 240",
            ),
            (
                "updates_per_second = 241",
                "updates_per_second",
                "241",
                "1 to 240",
            ),
            (
                "updates_per_second = -5",
                "updates_per_second",
                "-5",
                "1 to 240",
            ),
            (
                "updates_per_second = 4294967296",
                "updates_per_second",
                "4294967296",
                "1 to 240",
            ),
            (
                "updates_per_second = 60.5",
                "updates_per_second",
                "60.5",
                "integer",
            ),
            (
                "updates_per_second = 60.0",
                "updates_per_second",
                "60",
                "integer",
            ),
            (
                "updates_per_second = \"60\"",
                "updates_per_second",
                "",
                "integer",
            ),
            (
                "max_delta_seconds = 0",
                "max_delta_seconds",
                "0",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = 0.0",
                "max_delta_seconds",
                "0",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = -0.1",
                "max_delta_seconds",
                "-0.1",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = 1.5",
                "max_delta_seconds",
                "1.5",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = 2",
                "max_delta_seconds",
                "2",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = inf",
                "max_delta_seconds",
                "inf",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = nan",
                "max_delta_seconds",
                "NaN",
                "greater than 0 and at most 1",
            ),
            (
                "max_delta_seconds = \"0.1\"",
                "max_delta_seconds",
                "",
                "number",
            ),
        ] {
            let text = format!("[frame]\n{text}\n");
            let error = parse_config(&text).expect_err(&text);
            assert!(
                error.starts_with(PATH) && error.contains("line 2"),
                "{text:?}: {error}"
            );
            assert!(error.contains(key), "{text:?} names the key: {error}");
            assert!(error.contains(shown), "{text:?} shows the value: {error}");
            assert!(error.contains(range), "{text:?} states the range: {error}");
            assert!(!error.contains('\n'), "{text:?}: {error}");
        }
    }

    #[test]
    fn a_positive_max_delta_below_the_timer_resolution_is_refused_for_that_reason() {
        // Negative control: accepting the zero duration, or giving such a value the numeric-range wording, fails
        // here (the range sentence would claim 1e-12 is outside "greater than 0 and at most 1").
        // Below the minimum, whether it would round to zero (1e-12, 4e-10, 5e-10) or up to one nanosecond (0.999e-9 and the
        // next f64 below 1e-9): all refused, none by rounding.
        let below_one_nanosecond = f64::from_bits(1e-9_f64.to_bits() - 1);
        let next_below = format!("{below_one_nanosecond:e}");
        let next_below_shown = below_one_nanosecond.to_string();
        for (written, shown) in [
            ("1e-12", "0.000000000001"),
            ("4e-10", "0.0000000004"),
            ("5e-10", "0.0000000005"),
            ("9.99e-10", "0.000000000999"),
            ("0.0000000001", "0.0000000001"),
            (next_below.as_str(), next_below_shown.as_str()),
        ] {
            let text = format!("[frame]\nmax_delta_seconds = {written}\n");
            let error = parse_config(&text).expect_err(&text);
            assert!(
                error.starts_with(PATH) && error.contains("line 2"),
                "{error}"
            );
            assert!(
                error.contains("max_delta_seconds"),
                "names the key: {error}"
            );
            assert!(error.contains(shown), "shows the value: {error}");
            assert!(error.contains("timer resolution"), "states why: {error}");
            assert!(
                error.contains("one nanosecond"),
                "states the limit: {error}"
            );
            assert!(
                !error.contains("out of range"),
                "not the range wording: {error}"
            );
            assert!(!error.contains('\n'), "{error}");
        }
    }

    #[test]
    fn the_smallest_accepted_max_delta_is_exactly_one_nanosecond_and_range_errors_keep_their_wording()
     {
        let config = parse_config("[frame]\nmax_delta_seconds = 1e-9\n").expect("one nanosecond");
        assert_eq!(config.frame.max_delta, Duration::from_nanos(1));
        // Just above it is accepted too, as the nearest nanosecond.
        let above = f64::from_bits(1e-9_f64.to_bits() + 1);
        let config =
            parse_config(&format!("[frame]\nmax_delta_seconds = {above:e}\n")).expect("just above");
        assert_eq!(config.frame.max_delta, Duration::from_nanos(1));
        for text in [
            "max_delta_seconds = 0",
            "max_delta_seconds = 1.5",
            "max_delta_seconds = -1e-12",
        ] {
            let error = parse_config(&format!("[frame]\n{text}\n")).expect_err(text);
            assert!(error.contains("out of range"), "{text}: {error}");
            assert!(!error.contains("timer resolution"), "{text}: {error}");
        }
    }

    #[test]
    fn the_frame_table_rejects_unknown_keys_and_non_tables() {
        for text in [
            "[frame]\nrate = 60\n",
            "[frame]\nupdates_per_second = 60\nfps = 1\n",
            "frame = 60\n",
            "frame = [60, 0.1]\n",
            "frame = \"fast\"\n",
        ] {
            let error = parse_config(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
        }
    }

    #[test]
    fn the_runtime_table_rejects_unknown_keys_and_non_tables() {
        for text in [
            "[runtime]\ntimeout = 5\n",
            "[runtime]\nstart = 5\n",
            "runtime = []\n",
            "runtime = [5, 5, 1]\n",
            "runtime = 5\n",
            "runtime = \"fast\"\n",
        ] {
            let error = deadlines(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
        }
    }

    #[test]
    fn only_a_number_of_seconds_in_range_becomes_a_duration() {
        assert_eq!(
            Seconds::from_f64(0.2),
            Some(Seconds(Duration::from_millis(200)))
        );
        assert_eq!(
            Seconds::from_f64(60.0),
            Some(Seconds(Duration::from_secs(60)))
        );
        for bad in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0,
            -0.0,
            -1.0,
            60.000_001,
            f64::MAX,
            1e-300,
        ] {
            assert_eq!(Seconds::from_f64(bad), None, "{bad}");
        }
        assert_eq!(Seconds::from_u64(1), Some(Seconds(Duration::from_secs(1))));
        assert_eq!(
            Seconds::from_u64(60),
            Some(Seconds(Duration::from_secs(60)))
        );
        assert_eq!(Seconds::from_u64(0), None);
        assert_eq!(Seconds::from_u64(61), None);
        assert_eq!(Seconds::from_u64(u64::MAX), None);
    }

    // ---- the `lua` tables

    fn vms(text: &str) -> Result<Vec<(String, String)>, String> {
        parse_config(text).map(|config| {
            config
                .vms
                .into_iter()
                .map(|vm| (vm.name, vm.entry.display().to_string()))
                .collect()
        })
    }

    #[test]
    fn no_lua_table_means_no_vms() {
        assert_eq!(vms(""), Ok(Vec::new()));
        assert_eq!(vms("[log]\nlevel = \"warn\"\n"), Ok(Vec::new()));
        assert_eq!(vms("[lua]\n"), Ok(Vec::new()));
        assert_eq!(Config::default().vms, Vec::new());
    }

    #[test]
    fn vms_are_listed_in_name_order_with_entries_resolved_against_the_configuration_directory() {
        let text = "[lua.ui]\nentry = \"ui/main.lua\"\n\
                    [lua.gameplay]\nentry = \"../scripts/gameplay/main.lua\"\n\
                    [lua.a_1]\nentry = \"/abs/a.lua\"\n";
        assert_eq!(
            vms(text),
            Ok(vec![
                ("a_1".to_owned(), "/abs/a.lua".to_owned()),
                (
                    "gameplay".to_owned(),
                    "/tmp/codekarl-test/../scripts/gameplay/main.lua".to_owned()
                ),
                ("ui".to_owned(), "/tmp/codekarl-test/ui/main.lua".to_owned()),
            ])
        );
        // A configuration file named without a directory resolves against the working directory.
        let config = parse(
            Path::new("codekarl.toml"),
            "[lua.ui]\nentry = \"main.lua\"\n",
        )
        .expect("valid");
        assert_eq!(config.vms[0].entry, Path::new("main.lua"));
        // Inline tables work like standard ones.
        assert_eq!(
            vms("lua = { ui = { entry = \"m.lua\" } }\n"),
            Ok(vec![(
                "ui".to_owned(),
                "/tmp/codekarl-test/m.lua".to_owned()
            )])
        );
    }

    #[test]
    fn a_missing_entry_script_file_is_not_a_configuration_error() {
        assert!(vms("[lua.ui]\nentry = \"does/not/exist.lua\"\n").is_ok());
    }

    #[test]
    fn lua_tables_with_bad_shapes_names_or_keys_are_configuration_errors() {
        for text in [
            "[lua.ui]\n",            // no entry
            "[lua.ui]\nentry = 3\n", // a non-string entry
            "[lua.ui]\nentry = [\"m.lua\"]\n",
            "[lua.ui]\nentry = \"m.lua\"\nextra = 1\n", // an unknown key
            "[lua.ui]\nentry = \"m.lua\"\nconfig = {}\n",
            "[lua.UI]\nentry = \"m.lua\"\n", // invalid names
            "[lua.\"a-b\"]\nentry = \"m.lua\"\n",
            "[lua.\"\"]\nentry = \"m.lua\"\n",
            "[lua.\"a b\"]\nentry = \"m.lua\"\n",
            "[lua.\"ü\"]\nentry = \"m.lua\"\n",
            "lua = 3\n", // not a table of tables
            "lua = \"ui\"\n",
            "lua = []\n",
            "lua = [{ entry = \"m.lua\" }]\n",
            "[lua]\nui = 3\n",
            "[lua]\nui = \"m.lua\"\n",
            "[lua]\nui = []\n",
            "[lua]\nui = [\"m.lua\"]\n",
            "[lua]\nentry = \"m.lua\"\n",
        ] {
            let error = vms(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
            assert!(!error.contains('\n'), "{text:?}: {error}");
        }
        let name = vms("[lua.\"a-b\"]\nentry = \"m.lua\"\n").expect_err("invalid name");
        assert!(name.contains("a-b") && name.contains("lowercase"), "{name}");
        let key = vms("[lua.ui]\nentry = \"m.lua\"\nextra = 1\n").expect_err("unknown key");
        assert!(key.contains("line 3") && key.contains("extra"), "{key}");
    }

    // ---- the whole file as the VMs' `config`

    /// `text` as TOML, converted the way the whole file is.
    fn converted(text: &str) -> BTreeMap<String, ConfigValue> {
        let table = text.parse::<toml::Table>().expect("valid TOML");
        let ConfigValue::Table(whole) = to_config_value(&toml::Value::Table(table)) else {
            panic!("a table converts to a table");
        };
        whole
    }

    #[test]
    fn every_toml_type_converts_to_a_config_value() {
        let whole = converted(
            "title = \"codekarl\"\nanswer = 42\npi = 3.5\nflag = true\nmixed = [1, \"two\", 3.0]\n\
             when = 2026-10-19T10:11:12Z\nday = 2026-10-19\nclock = 10:11:12\n\
             [nested]\nlevel = \"warn\"\n[nested.deeper]\nok = false\n[[items]]\nid = 1\n[[items]]\nid = 2\n",
        );
        assert_eq!(whole["title"], ConfigValue::String("codekarl".to_owned()));
        assert_eq!(whole["answer"], ConfigValue::Integer(42));
        assert_eq!(whole["pi"], ConfigValue::Float(3.5));
        assert_eq!(whole["flag"], ConfigValue::Bool(true));
        assert_eq!(
            whole["mixed"],
            ConfigValue::Array(vec![
                ConfigValue::Integer(1),
                ConfigValue::String("two".to_owned()),
                ConfigValue::Float(3.0),
            ])
        );
        assert_eq!(
            whole["when"],
            ConfigValue::String("2026-10-19T10:11:12Z".to_owned())
        );
        assert_eq!(whole["day"], ConfigValue::String("2026-10-19".to_owned()));
        assert_eq!(whole["clock"], ConfigValue::String("10:11:12".to_owned()));
        let ConfigValue::Table(nested) = &whole["nested"] else {
            panic!("nested is a table");
        };
        assert_eq!(nested["level"], ConfigValue::String("warn".to_owned()));
        assert_eq!(
            nested["deeper"],
            ConfigValue::Table([("ok".to_owned(), ConfigValue::Bool(false))].into())
        );
        assert_eq!(
            whole["items"],
            ConfigValue::Array(vec![
                ConfigValue::Table([("id".to_owned(), ConfigValue::Integer(1))].into()),
                ConfigValue::Table([("id".to_owned(), ConfigValue::Integer(2))].into()),
            ])
        );
    }

    #[test]
    fn non_finite_floats_and_extreme_integers_survive_the_conversion() {
        let whole =
            converted("a = inf\nb = -inf\nc = 9223372036854775807\nd = -9223372036854775808\n");
        assert_eq!(whole["a"], ConfigValue::Float(f64::INFINITY));
        assert_eq!(whole["b"], ConfigValue::Float(f64::NEG_INFINITY));
        assert_eq!(whole["c"], ConfigValue::Integer(i64::MAX));
        assert_eq!(whole["d"], ConfigValue::Integer(i64::MIN));
        let nan = converted("n = nan\n");
        assert!(matches!(nan["n"], ConfigValue::Float(number) if number.is_nan()));
    }

    #[test]
    fn the_loaded_whole_file_holds_the_log_runtime_and_lua_tables_as_written() {
        let config = parse_config(
            "[log]\nlevel = \"warn\"\n[runtime]\nstop_timeout = 0.5\n[lua.ui]\nentry = \"m.lua\"\n",
        )
        .expect("valid");
        let ConfigValue::Table(whole) = &config.whole else {
            panic!("the whole file is a table: {:?}", config.whole);
        };
        assert_eq!(
            whole.keys().map(String::as_str).collect::<Vec<_>>(),
            ["log", "lua", "runtime"]
        );
        let ConfigValue::Table(runtime) = &whole["runtime"] else {
            panic!("runtime is a table");
        };
        assert_eq!(runtime["stop_timeout"], ConfigValue::Float(0.5));
        let ConfigValue::Table(lua) = &whole["lua"] else {
            panic!("lua is a table");
        };
        let ConfigValue::Table(ui) = &lua["ui"] else {
            panic!("lua.ui is a table");
        };
        assert_eq!(
            ui["entry"],
            ConfigValue::String("m.lua".to_owned()),
            "as written, not resolved"
        );
        assert_eq!(
            Config::default().whole,
            ConfigValue::Table(BTreeMap::new()),
            "the built-in defaults are an empty table"
        );
        assert_eq!(
            parse_config("").expect("empty").whole,
            Config::default().whole
        );
    }

    fn window_backend(text: &str) -> Result<Option<Backend>, String> {
        parse_config(text).map(|config| config.window_backend)
    }

    #[test]
    fn a_window_table_names_a_backend_or_none() {
        assert_eq!(window_backend(""), Ok(None));
        assert_eq!(
            window_backend("[window]\n"),
            Ok(None),
            "an empty table has no backend"
        );
        assert_eq!(
            window_backend("window = {}\n"),
            Ok(None),
            "also as an inline table"
        );
        for backend in [Backend::NativeMetal, Backend::GlfwVulkan] {
            assert_eq!(
                window_backend(&format!("[window]\nbackend = \"{}\"\n", backend.name())),
                Ok(Some(backend))
            );
        }
        // The whole file, `[window]` included, is still every VM's `config`.
        let whole = parse_config("[window]\nbackend = \"glfw-vulkan\"\n")
            .expect("valid")
            .whole;
        let ConfigValue::Table(entries) = whole else {
            panic!("a table");
        };
        assert!(entries.contains_key("window"));
    }

    #[test]
    fn window_errors_are_configuration_errors_with_the_path_and_the_line() {
        for (text, needle) in [
            ("[window]\nbackend = \"metal\"\n", "metal"),
            ("[window]\nbackend = 3\n", "integer `3`"),
            ("[window]\nbackend = true\n", "boolean"),
            ("[window]\nbackend = []\n", "sequence"),
            ("[window]\nbackend = \"\"\n", "unknown backend"),
            ("[window]\nbacked = \"glfw-vulkan\"\n", "backed"),
            ("[window]\ntitle = \"x\"\n", "title"),
            ("window = 3\n", "`window`"),
            ("window = \"native-metal\"\n", "`window`"),
            ("window = []\n", "`window`"),
            ("window = [{ backend = \"glfw-vulkan\" }]\n", "`window`"),
        ] {
            let error = window_backend(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
            assert!(error.contains("line "), "{text:?}: {error}");
            assert!(error.contains(needle), "{text:?}: {error}");
            assert!(!error.contains('\n'), "{text:?}: {error}");
        }
    }

    #[test]
    fn an_unavailable_backend_is_not_a_configuration_error_at_load() {
        // Availability is checked only for a windowed run (`mode::select`), never when loading.
        assert_eq!(
            window_backend("[window]\nbackend = \"native-metal\"\n"),
            Ok(Some(Backend::NativeMetal))
        );
    }

    fn input_bindings(text: &str) -> Result<Bindings, String> {
        parse_config(text).map(|config| config.bindings)
    }

    fn bindings_of(entries: &[(&str, &[&str])]) -> Bindings {
        Bindings::from_entries(entries.iter().map(|(action, keys)| {
            (
                (*action).to_owned(),
                keys.iter().map(|key| (*key).to_owned()).collect(),
            )
        }))
        .expect("valid bindings")
    }

    #[test]
    fn an_absent_input_table_gives_the_default_bindings() {
        let defaults = bindings_of(&[]);
        for text in [
            "",
            "[input]
",
            "[input.bindings]
",
        ] {
            assert_eq!(input_bindings(text), Ok(defaults.clone()), "{text:?}");
        }
        assert_eq!(Config::default().bindings, defaults);
    }

    #[test]
    fn a_valid_bindings_table_gives_those_bindings_and_defaults_for_the_rest() {
        let text = "[input.bindings]\nclose = [\"KeyQ\", \"Control+KeyW\"]\njump = [\"Space\"]\n";
        let parsed = input_bindings(text).expect("valid");
        assert_eq!(
            parsed,
            bindings_of(&[("close", &["KeyQ", "Control+KeyW"]), ("jump", &["Space"])])
        );
        assert!(
            parsed.action("move_left").is_some(),
            "the defaults fill the rest"
        );
        assert_eq!(
            parsed.action("close").map(|action| action.bindings().len()),
            Some(2)
        );
    }

    #[test]
    fn an_explicit_empty_list_unbinds_the_action() {
        let parsed = input_bindings("[input.bindings]\nclose = []\n").expect("valid");
        assert_eq!(
            parsed.action("close").map(|action| action.bindings().len()),
            Some(0),
            "Escape no longer closes"
        );
        assert!(
            parsed
                .action("move_left")
                .is_some_and(|action| !action.bindings().is_empty())
        );
    }

    #[test]
    fn every_ck_input_refusal_fails_the_load_with_its_message_naming_the_action_and_entry() {
        for (text, action, entry) in [
            ("[input.bindings]\nclose = [\"Esc\"]\n", "close", "Esc"),
            (
                "[input.bindings]\nclose = [\"Shift+Control+KeyA\"]\n",
                "close",
                "Shift+Control+KeyA",
            ),
            (
                "[input.bindings]\nclose = [\"KeyA\", \"KeyA\"]\n",
                "close",
                "KeyA",
            ),
            ("[input.bindings]\nBad = [\"KeyA\"]\n", "Bad", "Bad"),
            (
                "[input.bindings]\njump = [\"Control+ShiftLeft\"]\n",
                "jump",
                "Control+ShiftLeft",
            ),
            ("[input.bindings]\njump = [\"\"]\n", "jump", ""),
        ] {
            let error = input_bindings(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
            assert!(error.contains("[input.bindings]"), "{text:?}: {error}");
            assert!(
                error.contains(&format!("action {action:?}")),
                "{text:?}: {error}"
            );
            assert!(
                error.contains(&format!("entry {entry:?}")),
                "{text:?}: {error}"
            );
        }
        let nine = "[input.bindings]\nclose = [\"KeyA\",\"KeyB\",\"KeyC\",\"KeyD\",\"KeyE\",\"KeyF\",\"KeyG\",\"KeyH\",\"KeyI\"]\n";
        let error = input_bindings(nine).expect_err("nine bindings");
        assert!(error.contains("at most 8"), "{error}");
    }

    #[test]
    fn an_unknown_key_in_input_or_a_malformed_table_fails_the_load() {
        for (text, needle) in [
            ("[input]\nbindingz = {}\n", "bindingz"),
            ("[input]\ncolour_scheme = true\n", "colour_scheme"),
            ("[input]\nbindings = 3\n", "input.bindings"),
            ("input = 3\n", "`input`"),
            ("input = []\n", "`input`"),
            ("[input.bindings]\nclose = \"Escape\"\n", "sequence"),
            ("[input.bindings]\nclose = [3]\n", "string"),
        ] {
            let error = input_bindings(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
            assert!(error.contains(needle), "{text:?}: {error}");
        }
    }

    #[test]
    fn the_input_table_is_part_of_the_whole_file_every_vm_sees() {
        let config = parse_config("[input.bindings]\nclose = [\"KeyQ\"]\n").expect("valid");
        let ConfigValue::Table(entries) = &config.whole else {
            panic!("a table");
        };
        assert!(entries.contains_key("input"));
    }

    fn render(text: &str) -> Result<RenderConfig, String> {
        parse_config(text).map(|config| config.render)
    }

    #[test]
    fn an_absent_render_table_or_keys_give_nearest_and_the_assets_directory_beside_the_file() {
        for text in ["", "[render]\n", "[render]\nassets = \"assets\"\n"] {
            let config = render(text).expect(text);
            assert_eq!(config.filter, ImageFilter::Nearest, "{text:?}");
            assert_eq!(
                config.assets,
                Some(PathBuf::from("/tmp/codekarl-test/assets")),
                "{text:?}"
            );
        }
        // Without a configuration file there is no directory to be relative to.
        assert_eq!(Config::default().render, RenderConfig::default());
        assert_eq!(Config::default().render.assets, None);
    }

    #[test]
    fn each_valid_filter_and_a_relative_assets_directory_are_read() {
        for (name, filter) in [
            ("nearest", ImageFilter::Nearest),
            ("linear", ImageFilter::Linear),
        ] {
            let config = render(&format!("[render]\nfilter = \"{name}\"\n")).expect(name);
            assert_eq!(config.filter, filter);
        }
        let config = render("[render]\nassets = \"../pictures/png\"\n").expect("assets");
        assert_eq!(
            config.assets,
            Some(PathBuf::from("/tmp/codekarl-test/../pictures/png")),
            "relative to the configuration file's directory"
        );
        let config = render("[render]\nassets = \"/srv/art\"\n").expect("absolute");
        assert_eq!(
            config.assets,
            Some(PathBuf::from("/srv/art")),
            "an absolute path stands"
        );
    }

    #[test]
    fn render_errors_are_configuration_errors_with_the_path_and_the_line() {
        for (text, needle) in [
            ("[render]\nfilter = \"bilinear\"\n", "unknown filter"),
            ("[render]\nfilter = \"\"\n", "nearest"),
            ("[render]\nfilter = 3\n", "integer `3`"),
            ("[render]\nassets = 3\n", "integer `3`"),
            ("[render]\nassets = true\n", "boolean"),
            ("[render]\nfiltre = \"linear\"\n", "filtre"),
            ("[render]\nscale = 2\n", "scale"),
            ("render = 3\n", "`render`"),
            ("render = \"linear\"\n", "`render`"),
            ("render = []\n", "`render`"),
        ] {
            let error = render(text).expect_err(text);
            assert!(error.starts_with(PATH), "{text:?}: {error}");
            assert!(error.contains("line "), "{text:?}: {error}");
            assert!(error.contains(needle), "{text:?}: {error}");
            assert!(!error.contains('\n'), "{text:?}: {error}");
        }
    }

    #[test]
    fn the_render_table_is_part_of_the_whole_file_every_vm_sees() {
        let whole = parse_config("[render]\nfilter = \"linear\"\n")
            .expect("valid")
            .whole;
        let ConfigValue::Table(entries) = whole else {
            panic!("a table");
        };
        assert!(entries.contains_key("render"));
    }

    #[test]
    fn game_names_a_scenario_and_defaults_the_saves_directory_beside_the_file() {
        let config = parse_config("[game]\nscenario = \"scenario\"\n").unwrap();
        assert_eq!(
            config.game,
            Some(GameConfig {
                scenario: PathBuf::from("/tmp/codekarl-test/scenario"),
                saves: PathBuf::from("/tmp/codekarl-test/saves"),
            })
        );
        // The scenario is the map source, with the default camera rates.
        assert_eq!(
            config.map,
            Some(MapConfig {
                directory: PathBuf::from("/tmp/codekarl-test/scenario"),
                pan_pixels_per_frame: 4,
                zoom_steps_per_second: 20,
                scroll_points_per_step: 24,
            })
        );
        let custom =
            parse_config("[game]\nscenario = \"s\"\nsaves = \"elsewhere/slots\"\n[map]\npan_pixels_per_frame = 7\n")
                .unwrap();
        assert_eq!(
            custom.game.unwrap().saves,
            PathBuf::from("/tmp/codekarl-test/elsewhere/slots")
        );
        assert_eq!(custom.map.unwrap().pan_pixels_per_frame, 7);
    }

    #[test]
    fn no_game_table_means_no_simulation() {
        assert_eq!(parse_config("").unwrap().game, None);
        assert_eq!(
            parse_config("[map]\ndirectory = \"m\"\n").unwrap().game,
            None
        );
    }

    #[test]
    fn game_without_a_scenario_or_with_an_unknown_key_is_refused() {
        let missing = parse_config("[game]\nsaves = \"s\"\n").unwrap_err();
        assert!(
            missing.contains("[game]: scenario is required"),
            "{missing}"
        );
        let empty = parse_config("[game]\n").unwrap_err();
        assert!(empty.contains("[game]"), "{empty}");
        let unknown = parse_config("[game]\nscenario = \"s\"\nspeed = 3\n").unwrap_err();
        assert!(unknown.contains("speed"), "{unknown}");
        assert!(parse_config("game = 3\n").is_err());
        assert!(parse_config("[game]\nscenario = 3\n").is_err());
    }

    #[test]
    fn a_scenario_and_a_map_directory_cannot_both_be_set() {
        let error =
            parse_config("[game]\nscenario = \"s\"\n[map]\ndirectory = \"m\"\n").unwrap_err();
        assert!(
            error.contains("[game] scenario and [map] directory cannot both be set"),
            "{error}"
        );
    }

    #[test]
    fn typed_text_adds_no_bound_action_a_games_bindings_are_the_22_of_today() {
        // CK-314: a digit, a letter or an editing key reaches a widget as a typed entry, never as an action. The game's actions are the
        // loop's 6 (the map's pan and zoom among them) and the 10 time and save actions.
        let config = parse_config("[game]\nscenario = \"s\"\n").unwrap();
        let names: Vec<_> = config
            .bindings
            .actions()
            .iter()
            .map(ck_window::ActionBindings::name)
            .collect();
        assert_eq!(names.len(), 22, "{names:?}");
        assert!(
            names.iter().all(|name| ![
                "type",
                "typed",
                "text",
                "backspace",
                "enter",
                "tab",
                "edit_left",
                "edit_right"
            ]
            .contains(name)),
            "{names:?}"
        );
    }

    #[test]
    fn the_time_and_save_actions_have_defaults_only_with_a_scenario() {
        let actions = [
            ("time_pause", "Space"),
            ("time_speed_1", "Digit1"),
            ("time_speed_2", "Digit2"),
            ("time_speed_3", "Digit3"),
            ("time_speed_4", "Digit4"),
            ("time_speed_5", "Digit5"),
            ("time_faster", "BracketRight"),
            ("time_slower", "BracketLeft"),
            ("game_quicksave", "F5"),
            ("game_quickload", "F9"),
        ];
        for text in ["", "[map]\ndirectory = \"m\"\n"] {
            let config = parse_config(text).unwrap();
            for (action, _) in actions {
                assert!(config.bindings.action(action).is_none(), "{action}");
            }
        }
        let config = parse_config("[game]\nscenario = \"s\"\n").unwrap();
        for (action, key) in actions {
            let bindings = config.bindings.action(action).unwrap().bindings();
            assert_eq!(bindings.len(), 1, "{action}");
            assert_eq!(bindings[0].key().name(), key, "{action}");
        }
        // The map's defaults are still there, and a configured binding replaces a default.
        assert!(config.bindings.action("map_pan_left").is_some());
        let rebound = parse_config(
            "[game]\nscenario = \"s\"\n[input.bindings]\ntime_pause = [\"KeyP\"]\ngame_quickload = []\n",
        )
        .unwrap();
        assert_eq!(
            rebound.bindings.action("time_pause").unwrap().bindings()[0]
                .key()
                .name(),
            "KeyP"
        );
        assert!(
            rebound
                .bindings
                .action("game_quickload")
                .unwrap()
                .bindings()
                .is_empty()
        );
    }

    #[test]
    fn ui_scale_is_one_one_and_a_half_or_two_and_unknown_ui_keys_are_refused() {
        for (text, k) in [
            ("[ui]\nscale = 1\n", Some(2)),
            ("[ui]\nscale = 1.5\n", Some(3)),
            ("[ui]\nscale = 2\n", Some(4)),
            ("[ui]\nscale = 2.0\n", Some(4)),
            ("[ui]\nscale = 1.0\n", Some(2)),
            ("", None),
            ("[ui]\n", None),
        ] {
            let config = parse_config(text).unwrap();
            assert_eq!(config.ui_scale_limit_k, k, "{text:?}");
        }
        for bad in ["0", "3", "1.25", "\"2\"", "-1", "true"] {
            let refused = parse_config(&format!("[ui]\nscale = {bad}\n")).unwrap_err();
            assert!(
                refused.contains("[ui]") && refused.contains("use 1, 1.5 or 2"),
                "{bad}: {refused}"
            );
        }
        let unknown = parse_config("[ui]\ncolour = 1\n").unwrap_err();
        assert!(unknown.contains("colour"), "{unknown}");
        assert!(parse_config("ui = 3\n").is_err());
    }

    #[test]
    fn points_per_pixel_is_retired_and_the_refusal_names_the_new_key() {
        for value in ["2", "1", "9", "\"x\""] {
            let refused = parse_config(&format!("[ui]\npoints_per_pixel = {value}\n")).unwrap_err();
            assert!(
                refused.contains("points_per_pixel is retired") && refused.contains("scale"),
                "{value}: {refused}"
            );
        }
    }

    #[test]
    fn the_old_font_scale_key_is_refused_naming_the_new_one_alone_and_beside_it() {
        let expected = "[ui]: font_points_per_pixel was renamed: use scale (1, 1.5 or 2)";
        for text in [
            "[ui]\nfont_points_per_pixel = 2\n",
            "[ui]\nfont_points_per_pixel = 2\npoints_per_pixel = 2\n",
            "[ui]\npoints_per_pixel = 2\nfont_points_per_pixel = 9\n",
            "[ui]\nfont_points_per_pixel = \"x\"\n",
        ] {
            let refused = parse_config(text).unwrap_err();
            assert!(refused.ends_with(expected), "{text:?}: {refused}");
            assert!(
                refused.starts_with("/tmp/codekarl-test/config.toml: "),
                "{text:?}: {refused}"
            );
        }
    }

    #[test]
    fn a_scenario_requires_nearest_sampling_and_names_both_keys_otherwise() {
        let game = "[game]\nscenario = \"s\"\n";
        parse_config(game).unwrap();
        parse_config(&format!("{game}[render]\nfilter = \"nearest\"\n")).unwrap();
        let refused = parse_config(&format!("{game}[render]\nfilter = \"linear\"\n")).unwrap_err();
        assert!(
            refused.contains("[render] filter") && refused.contains("[game] scenario"),
            "{refused}"
        );
        // Without a scenario nothing changes.
        let linear = parse_config("[render]\nfilter = \"linear\"\n").unwrap();
        assert_eq!(linear.render.filter, ImageFilter::Linear);
    }
}
