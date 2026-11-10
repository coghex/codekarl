//! The `window` extension of the UI VM (design §6): the Lua surface of the window request slot.
//!
//! [`extension`] builds the `ck-lua` [`Extension`] the VM named `ui` gets; no other VM does. Its host
//! functions run on the UI VM's owner thread, write the slot (`ck-window`'s [`RequestWriter`]: a latest-value
//! slot, never waiting for the main thread) and return at once:
//!
//! - `window.open{title=, width=, height=, clear={r, g, b}, unit=, resizable=, min_width=, min_height=}`: the one-shot `open`. `unit` is
//!   `"points"` (the default) or `"pixels"` (resolved once against the main screen); a resizable window may have a minimum. It returns the
//!   width and height in points the window opens at. A bad description or a second `open` raises an ordinary Lua error with the slot's
//!   reason;
//! - `window.set_clear(r, g, b)`: the colour (finite components are clamped to 0..=1, a non-finite one
//!   raises an error);
//! - `window.close()`: the one-shot `close`;
//! - `window.on_close_request(fn)` and `window.on_opened(fn)`: the callbacks `close_request` and `opened`.
//!
//! **Headless runs** get the same extension over a real slot, so every call is validated the same way and
//! in the same order as in a windowed run (the argument checks here, then the slot's size bounds, finite
//! colours and one-shot `open`). Only the reading side differs: nothing reads the slot. The first
//! *accepted* `open` logs one `info window: window.open ignored: headless run` record, and every accepted
//! write is counted ([`HeadlessNotes`]) for the one `debug` record at the end of the run. The `render` extension
//! ([`crate::render_api`]) shares the same notes: its first accepted call logs one `info render:` record and its
//! further accepted calls are counted in the same final record.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use ck_log::Level;
use ck_lua::{ConfigValue, Extension, HostFunction};
use ck_window::{MAX_DIMENSION, RequestWriter, ScreenSnapshot, SizeUnit, WindowDesc};

/// The name of the callback `window.on_close_request` registers.
pub(crate) const CLOSE_REQUEST: &str = "close_request";
/// The name of the callback `window.on_opened` registers.
pub(crate) const OPENED: &str = "opened";
/// The name of the callback a paced UI update is posted as (the Lua `frame.on_update` of milestone 5-8 registers it).
pub(crate) const FRAME_UPDATE: &str = "frame_update";

/// The main screen as the main thread saw it once, before the UI VM was spawned. Empty in a run with no window system (a headless
/// run, or a backend that gave none), where a size in pixels resolves at one pixel per point with no limit on the screen's size.
pub(crate) type ScreenCell = Arc<OnceLock<ScreenSnapshot>>;

/// The record text of the first accepted `open` in a headless run.
const IGNORED: &str = "window.open ignored: headless run";
/// The record text of the first accepted `render` call in a headless run.
const RENDER_IGNORED: &str = "scene calls ignored: headless run";

/// Where a headless run's records go: the level, the category and the text (a parameter so that tests can
/// count the records).
type Sink = Box<dyn Fn(Level, &'static str, &str) + Send + Sync>;

/// What a headless run notes about the window calls it ignored.
pub(crate) struct HeadlessNotes {
    sink: Sink,
    /// The first accepted `open` was logged.
    logged: AtomicBool,
    /// The accepted writes after the first record, for the final `debug` count.
    ignored: AtomicU64,
    /// The first accepted `render` call was logged.
    render_logged: AtomicBool,
    /// The accepted `render` calls after the first record, for the final `debug` count.
    render_ignored: AtomicU64,
}

impl std::fmt::Debug for HeadlessNotes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeadlessNotes")
            .field("logged", &self.logged.load(Ordering::SeqCst))
            .field("ignored", &self.ignored.load(Ordering::SeqCst))
            .field("render_logged", &self.render_logged.load(Ordering::SeqCst))
            .field(
                "render_ignored",
                &self.render_ignored.load(Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

impl HeadlessNotes {
    /// Notes that log through the installed logger, category `window`, with the waiting call (the caller
    /// is the UI VM's owner thread, an ordinary producer).
    pub(crate) fn logging() -> Self {
        Self::with_sink(Box::new(|level, category, message| match level {
            Level::Info => ck_log::info!(category, "{message}"),
            _ => ck_log::debug!(category, "{message}"),
        }))
    }

    /// Notes that send their one record to `sink`.
    pub(crate) fn with_sink(sink: Sink) -> Self {
        Self {
            sink,
            logged: AtomicBool::new(false),
            ignored: AtomicU64::new(0),
            render_logged: AtomicBool::new(false),
            render_ignored: AtomicU64::new(0),
        }
    }

    /// An accepted `open`: the first logs the one `info` record.
    fn accepted_open(&self) {
        if !self.logged.swap(true, Ordering::SeqCst) {
            (self.sink)(Level::Info, "window", IGNORED);
        }
    }

    /// An accepted `set_clear` or `close`: counted, never logged one by one.
    fn accepted_write(&self) {
        self.ignored.fetch_add(1, Ordering::SeqCst);
    }

    /// An accepted `render.quad` or `render.clear_quad`: the first logs the one `info render:` record, every
    /// later one is counted.
    pub(crate) fn accepted_render(&self) {
        if self.render_logged.swap(true, Ordering::SeqCst) {
            self.render_ignored.fetch_add(1, Ordering::SeqCst);
        } else {
            (self.sink)(Level::Info, "render", RENDER_IGNORED);
        }
    }

    /// The text of the final `debug` record, if any window or render call was accepted in this run.
    pub(crate) fn final_count(&self) -> Option<String> {
        let windows = self.ignored.load(Ordering::SeqCst);
        let renders = self.render_ignored.load(Ordering::SeqCst);
        let any_window = self.logged.load(Ordering::SeqCst) || windows > 0;
        let any_render = self.render_logged.load(Ordering::SeqCst) || renders > 0;
        match (any_window, any_render) {
            (false, false) => None,
            (true, false) => Some(format!(
                "{windows} further window request(s) ignored: headless run"
            )),
            (false, true) => Some(format!(
                "{renders} further render request(s) ignored: headless run"
            )),
            (true, true) => Some(format!(
                "{windows} further window request(s) and {renders} further render request(s) ignored: headless run"
            )),
        }
    }
}

/// A number argument (`nil`, a boolean, a string... are errors).
pub(crate) fn number(value: &ConfigValue) -> Option<f64> {
    match value {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a colour component or a window size: the precision of an f64 is ample"
        )]
        ConfigValue::Integer(value) => Some(*value as f64),
        ConfigValue::Float(value) => Some(*value),
        _ => None,
    }
}

/// A colour component as the slot takes it: a finite value is clamped to 0..=1 first (so no finite f64
/// overflows the narrower float), a non-finite one stays non-finite for the slot to refuse.
fn component(value: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a finite value was clamped to 0..=1; a non-finite one keeps its kind"
    )]
    let narrowed = if value.is_finite() {
        value.clamp(0.0, 1.0) as f32
    } else {
        value as f32
    };
    narrowed
}

/// Three colour components from `window.set_clear`'s arguments or `window.open`'s `clear` array.
fn colour(values: &[Option<ConfigValue>], what: &str) -> Result<[f32; 3], String> {
    let mut rgb = [0.0_f32; 3];
    if values.len() != 3 {
        return Err(format!("{what} needs three numbers (r, g, b)"));
    }
    for (index, (slot, value)) in rgb.iter_mut().zip(values).enumerate() {
        let Some(value) = value.as_ref().and_then(number) else {
            return Err(format!("{what}: component {index} is not a number"));
        };
        *slot = component(value);
    }
    Ok(rgb)
}

/// A dimension: an integer or an integral float, which must fit the slot's bounds (the slot's own message
/// names the bound, so the check is the slot's).
fn dimension(table: &BTreeMap<String, ConfigValue>, name: &str) -> Result<u32, String> {
    let value = match table.get(name) {
        Some(ConfigValue::Integer(value)) => *value,
        Some(ConfigValue::Float(value)) if value.fract() == 0.0 && value.abs() < 1.0e15 => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an integral value below 1e15 converts exactly"
            )]
            let exact = *value as i64;
            exact
        }
        Some(_) => return Err(format!("window.open: `{name}` must be an integer")),
        None => return Err(format!("window.open: `{name}` is missing")),
    };
    // A value that does not fit a `u32` is out of the slot's bounds too; say so in the slot's words.
    u32::try_from(value).map_err(|_| {
        format!("invalid window description: {name} {value} is not from 1 to {MAX_DIMENSION}")
    })
}

/// A minimum from `window.open`'s table: both `min_width` and `min_height`, or neither.
fn minimum(table: &BTreeMap<String, ConfigValue>) -> Result<Option<(u32, u32)>, String> {
    match (
        table.contains_key("min_width"),
        table.contains_key("min_height"),
    ) {
        (false, false) => Ok(None),
        (true, true) => Ok(Some((
            dimension(table, "min_width")?,
            dimension(table, "min_height")?,
        ))),
        (true, false) => Err("window.open: `min_width` needs `min_height` too".into()),
        (false, true) => Err("window.open: `min_height` needs `min_width` too".into()),
    }
}

/// The window description `window.open`'s table describes (the slot validates the values): sizes in `unit` ("points", the default,
/// or "pixels"), `resizable` (default false), and `min_width` with `min_height` for a resizable window.
fn description(arguments: &[Option<ConfigValue>]) -> Result<WindowDesc, String> {
    let [Some(ConfigValue::Table(table))] = arguments else {
        return Err(
            "window.open: expected one table {title=, width=, height=, clear={r, g, b}}".into(),
        );
    };
    if let Some(unknown) = table.keys().find(|key| {
        ![
            "title",
            "width",
            "height",
            "clear",
            "unit",
            "resizable",
            "min_width",
            "min_height",
        ]
        .contains(&key.as_str())
    }) {
        return Err(format!("window.open: unknown field `{unknown}`"));
    }
    let title = match table.get("title") {
        Some(ConfigValue::String(title)) => title.clone(),
        Some(_) => return Err("window.open: `title` must be a string".into()),
        None => return Err("window.open: `title` is missing".into()),
    };
    let clear = match table.get("clear") {
        Some(ConfigValue::Array(values)) => {
            let values: Vec<_> = values.iter().cloned().map(Some).collect();
            colour(&values, "window.open: `clear`")?
        }
        Some(_) => return Err("window.open: `clear` must be an array {r, g, b}".into()),
        None => return Err("window.open: `clear` is missing".into()),
    };
    let unit = match table.get("unit") {
        None => SizeUnit::Points,
        Some(ConfigValue::String(name)) if name == "points" => SizeUnit::Points,
        Some(ConfigValue::String(name)) if name == "pixels" => SizeUnit::Pixels,
        Some(_) => return Err("window.open: `unit` must be \"points\" or \"pixels\"".into()),
    };
    let resizable = match table.get("resizable") {
        None => false,
        Some(ConfigValue::Bool(value)) => *value,
        Some(_) => return Err("window.open: `resizable` must be true or false".into()),
    };
    Ok(WindowDesc {
        title,
        width: dimension(table, "width")?,
        height: dimension(table, "height")?,
        clear,
        unit,
        resizable,
        min: minimum(table)?,
        pixel_request: None,
    })
}

/// What the three host functions share.
#[derive(Clone)]
struct Shared {
    writer: RequestWriter,
    headless: Option<Arc<HeadlessNotes>>,
    map: Option<crate::map_app::SharedController>,
    /// The main screen, snapshotted on the main thread before this VM was spawned. The binding resolves sizes against it and never
    /// calls a backend.
    screen: ScreenCell,
}

impl Shared {
    fn open(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let requested = description(arguments)?;
        let screen = self
            .screen
            .get()
            .copied()
            .unwrap_or_else(ScreenSnapshot::unlimited);
        // One resolution, here, on the VM's thread: every consumer below receives this one description, in points.
        let resolved = requested
            .resolve(&screen)
            .map_err(|error| error.to_string())?;
        for line in &resolved.clamps {
            ck_log::info!("window", "{line}");
        }
        let desc = resolved.desc;
        let size = (desc.width, desc.height);
        if let Some(map) = &self.map {
            crate::map_app::lock(map).open(&desc)?;
        }
        self.writer.open(desc).map_err(|error| error.to_string())?;
        if let Some(notes) = &self.headless {
            notes.accepted_open();
        }
        // The size the window opens at, in points: a window asked for in pixels has a size in points that depends on the display, and the
        // script's first layout starts from it.
        Ok(vec![
            ConfigValue::Integer(i64::from(size.0)),
            ConfigValue::Integer(i64::from(size.1)),
        ])
    }

    fn set_clear(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let rgb = colour(arguments, "window.set_clear")?;
        self.writer
            .set_clear(rgb)
            .map_err(|error| format!("window.set_clear: {error}"))?;
        if let Some(notes) = &self.headless {
            notes.accepted_write();
        }
        Ok(Vec::new())
    }

    fn close(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        if !arguments.is_empty() {
            return Err("window.close takes no arguments".into());
        }
        self.writer.close();
        if let Some(notes) = &self.headless {
            notes.accepted_write();
        }
        Ok(Vec::new())
    }
}

/// The `window` extension over `writer`, for a run with no screen snapshot (sizes in pixels resolve at one pixel per point with no
/// limit). `headless` is `Some` in a headless run: the same functions, the same validation, plus the notes of what was ignored.
#[cfg(test)]
pub(crate) fn extension(
    writer: RequestWriter,
    headless: Option<Arc<HeadlessNotes>>,
    map: Option<crate::map_app::SharedController>,
) -> Extension {
    extension_with_screen(writer, headless, map, ScreenCell::default())
}

/// [`extension`] with the main screen's cell: the main thread fills it before the UI VM is spawned, and `window.open` reads it.
pub(crate) fn extension_with_screen(
    writer: RequestWriter,
    headless: Option<Arc<HeadlessNotes>>,
    map: Option<crate::map_app::SharedController>,
    screen: ScreenCell,
) -> Extension {
    let shared = Shared {
        writer,
        headless,
        map,
        screen,
    };
    let open: HostFunction = {
        let shared = shared.clone();
        Arc::new(move |arguments| shared.open(arguments))
    };
    let set_clear: HostFunction = {
        let shared = shared.clone();
        Arc::new(move |arguments| shared.set_clear(arguments))
    };
    let close: HostFunction = Arc::new(move |arguments| shared.close(arguments));
    Extension::new("window")
        .function("open", open)
        .function("set_clear", set_clear)
        .function("close", close)
        .callback("on_close_request", CLOSE_REQUEST)
        .callback("on_opened", OPENED)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use ck_lua::{CallOutcome, VmOwner, VmSettings};
    use ck_window::{Notifier, Requests};

    use super::*;

    /// A scratch directory for scripts, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "codekarl-window-api-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The slot, a reader for it, and the records the headless notes wrote.
    struct Rig {
        requests: Requests,
        headless: Option<Arc<HeadlessNotes>>,
        records: Arc<Mutex<Vec<(Level, String)>>>,
        extension: Extension,
    }

    impl Rig {
        fn new(headless: bool) -> Self {
            let requests = Requests::new();
            let records = Arc::new(Mutex::new(Vec::new()));
            let notes = headless.then(|| {
                let records = Arc::clone(&records);
                Arc::new(HeadlessNotes::with_sink(Box::new(
                    move |level, _category, text| {
                        records
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push((level, text.to_owned()));
                    },
                )))
            });
            Self::with_extension(requests, notes, records, |writer, notes| {
                extension(writer, notes, None)
            })
        }

        /// A rig whose extension was given the main screen the application took on the main thread, as the production wiring does.
        fn with_screen(screen: ScreenSnapshot) -> Self {
            let requests = Requests::new();
            let records = Arc::new(Mutex::new(Vec::new()));
            let cell = ScreenCell::default();
            cell.set(screen).expect("empty");
            Self::with_extension(requests, None, records, |writer, notes| {
                extension_with_screen(writer, notes, None, cell)
            })
        }

        fn with_extension(
            requests: Requests,
            notes: Option<Arc<HeadlessNotes>>,
            records: Arc<Mutex<Vec<(Level, String)>>>,
            build: impl FnOnce(RequestWriter, Option<Arc<HeadlessNotes>>) -> Extension,
        ) -> Self {
            let writer = requests.writer(Notifier::new());
            Self {
                extension: build(writer, notes.clone()),
                requests,
                headless: notes,
                records,
            }
        }

        fn records(&self) -> Vec<(Level, String)> {
            self.records
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// Runs `body` (Lua statements returning values) inside the `opened` callback of a VM that has the
    /// extension and returns what it returned.
    fn lua(rig: &Rig, body: &str) -> Vec<Option<ConfigValue>> {
        let dir = Dir::new();
        let entry = dir.0.join("main.lua");
        fs::write(
            &entry,
            format!("window.on_opened(function()\n{body}\nend)\n"),
        )
        .expect("a script");
        let mut settings = VmSettings::new("test", entry, ConfigValue::Table(BTreeMap::new()));
        settings.extensions = vec![rig.extension.clone()];
        settings.call_limit = 4;
        let mut vm = VmOwner::spawn(settings).expect("the VM spawns");
        vm.wait_ready(Duration::from_secs(5)).expect("ready");
        vm.start(Duration::from_secs(5)).expect("started");
        vm.post(OPENED, Vec::new()).expect("posted");
        let give_up = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if let Some(result) = vm.take_result() {
                break result;
            }
            assert!(Instant::now() < give_up, "the callback never answered");
            std::thread::sleep(Duration::from_millis(1));
        };
        let CallOutcome::Returned(values) = result.outcome else {
            panic!("the callback did not return: {:?}", result.outcome);
        };
        let _ = vm.stop(Duration::from_secs(5));
        values
    }

    fn text(value: Option<&ConfigValue>) -> String {
        match value {
            Some(ConfigValue::String(text)) => text.clone(),
            other => panic!("not a string: {other:?}"),
        }
    }

    /// The error text of `pcall(call)` for a Lua expression `call`, or `ok`. The traceback (which names the
    /// scratch directory) is cut off.
    fn error_of(rig: &Rig, call: &str) -> String {
        let values = lua(
            rig,
            &format!(
                "local ok, err = pcall(function() {call} end)\nif ok then return 'ok' end\nreturn tostring(err)"
            ),
        );
        let full = text(values[0].as_ref());
        full.split("\nstack traceback")
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    const OPEN: &str = "window.open{title='t', width=200, height=100, clear={0.1, 0.2, 0.3}}";

    #[test]
    fn open_set_clear_and_close_write_the_slot() {
        let rig = Rig::new(false);
        assert_eq!(error_of(&rig, OPEN), "ok");
        assert_eq!(
            error_of(&rig, "window.set_clear(0.5, 2, -1)"),
            "ok",
            "finite components are clamped"
        );
        assert_eq!(error_of(&rig, "window.close()"), "ok");
        let changes = rig.requests.reader().take();
        let desc = changes.open.expect("the open request");
        assert_eq!(
            (desc.title.as_str(), desc.width, desc.height),
            ("t", 200, 100)
        );
        assert!(
            desc.clear
                .iter()
                .zip([0.1_f32, 0.2, 0.3])
                .all(|(got, wanted)| (got - wanted).abs() < 1e-6),
            "{:?}",
            desc.clear
        );
        assert_eq!(changes.clear, Some([0.5, 1.0, 0.0]));
        assert!(changes.close);
    }

    #[test]
    fn invalid_calls_raise_ordinary_lua_errors_with_the_reason() {
        let rig = Rig::new(false);
        for (call, wanted) in [
            ("window.open()", "expected one table"),
            ("window.open(3)", "expected one table"),
            ("window.open{}", "`title` is missing"),
            (
                "window.open{title=3, width=1, height=1, clear={0,0,0}}",
                "`title` must be a string",
            ),
            (
                "window.open{title='t', height=1, clear={0,0,0}}",
                "`width` is missing",
            ),
            (
                "window.open{title='t', width='1', height=1, clear={0,0,0}}",
                "`width` must be an integer",
            ),
            (
                "window.open{title='t', width=1, height=1}",
                "`clear` is missing",
            ),
            (
                "window.open{title='t', width=1, height=1, clear={0,0}}",
                "needs three numbers",
            ),
            (
                "window.open{title='t', width=1, height=1, clear={0,0,'x'}}",
                "component 2 is not a number",
            ),
            (
                "window.open{title='t', width=1, height=1, clear={0,0,0}, extra=1}",
                "unknown field `extra`",
            ),
            (
                "window.open{title='t', width=0, height=1, clear={0,0,0}}",
                "width 0 is not from 1 to 16384",
            ),
            (
                "window.open{title='t', width=1, height=99999, clear={0,0,0}}",
                "height 99999 is not from 1 to 16384",
            ),
            (
                "window.open{title='t', width=-4, height=1, clear={0,0,0}}",
                "width -4 is not from 1 to 16384",
            ),
            (
                "window.open{title='t', width=1, height=1, clear={0,0,1/0}}",
                "colour component 2 is not finite",
            ),
            ("window.set_clear(0.1, 0.2)", "needs three numbers"),
            (
                "window.set_clear(0.1, 'x', 0.2)",
                "component 1 is not a number",
            ),
            (
                "window.set_clear(0, 0, 0/0)",
                "colour component 2 is not finite",
            ),
            ("window.close(1)", "takes no arguments"),
        ] {
            let error = error_of(&rig, call);
            assert!(error.contains(wanted), "{call}: {error}");
        }
        // Nothing invalid was recorded.
        let changes = rig.requests.reader().take();
        assert!(changes.open.is_none() && changes.clear.is_none() && !changes.close);
    }

    #[test]
    fn a_second_open_is_an_error_even_when_the_first_was_accepted_long_ago() {
        let rig = Rig::new(false);
        assert_eq!(error_of(&rig, OPEN), "ok");
        assert!(error_of(&rig, OPEN).contains("the window was already opened"));
    }

    #[test]
    fn the_callbacks_are_registered_by_the_two_setters() {
        let rig = Rig::new(false);
        let values = lua(
            &rig,
            "window.on_close_request(function(source) return source end)\nreturn 'set'",
        );
        assert_eq!(text(values[0].as_ref()), "set");
        // A setter given something that is not a function or nil is an argument error.
        assert!(error_of(&rig, "window.on_opened(3)").contains("function"));
    }

    #[test]
    fn headless_runs_validate_exactly_like_windowed_ones_and_log_the_first_accepted_open_once() {
        let windowed = Rig::new(false);
        let headless = Rig::new(true);
        for call in [
            "window.open{title='t', width=0, height=1, clear={0,0,0}}",
            "window.open{title='t', width=1, height=1, clear={0,0,1/0}}",
            "window.open()",
            "window.set_clear(0, 0, 0/0)",
        ] {
            assert_eq!(
                error_of(&windowed, call),
                error_of(&headless, call),
                "{call}"
            );
        }
        assert!(headless.records().is_empty(), "rejected calls log nothing");
        assert_eq!(error_of(&headless, OPEN), "ok");
        assert_eq!(
            headless.records(),
            [(Level::Info, IGNORED.to_owned())],
            "the first accepted open logs once"
        );
        // A second valid open is an error in both, and logs nothing more.
        assert_eq!(
            error_of(&windowed, OPEN),
            "ok",
            "the windowed run accepts its first open"
        );
        assert_eq!(error_of(&windowed, OPEN), error_of(&headless, OPEN));
        assert!(error_of(&headless, OPEN).contains("already opened"));
        // Later accepted writes log nothing; they are counted.
        for call in [
            "window.set_clear(1, 1, 1)",
            "window.set_clear(0, 0, 0)",
            "window.close()",
        ] {
            assert_eq!(error_of(&headless, call), "ok");
        }
        assert_eq!(headless.records().len(), 1, "{:?}", headless.records());
        let notes = headless.headless.as_ref().expect("the notes");
        assert_eq!(
            notes.final_count().as_deref(),
            Some("3 further window request(s) ignored: headless run")
        );
    }

    #[test]
    fn a_headless_run_that_never_called_the_window_api_has_nothing_to_count() {
        let notes = HeadlessNotes::with_sink(Box::new(|_, _, _| {}));
        assert_eq!(notes.final_count(), None);
        notes.accepted_write();
        assert!(notes.final_count().is_some());
    }

    #[test]
    fn the_extension_is_valid_for_ck_lua_and_only_the_ui_vm_is_given_it() {
        // `VmOwner::spawn` refuses an invalid extension, so a successful spawn is the check.
        let rig = Rig::new(false);
        let values = lua(&rig, "return type(window.open), type(window.close)");
        assert_eq!(text(values[0].as_ref()), "function");
        // A VM without the extension has no `window` global at all.
        let dir = Dir::new();
        let entry = dir.0.join("main.lua");
        fs::write(&entry, "function startup() assert(window == nil) end").expect("a script");
        let settings = VmSettings::new("gameplay", entry, ConfigValue::Table(BTreeMap::new()));
        let mut vm = VmOwner::spawn(settings).expect("the VM spawns");
        vm.wait_ready(Duration::from_secs(5)).expect("ready");
        vm.start(Duration::from_secs(5)).expect("no window global");
        let _ = vm.stop(Duration::from_secs(5));
    }

    #[test]
    fn a_window_in_pixels_resolves_against_the_screen_the_application_took_not_a_backend() {
        // A fake snapshot at 2 pixels a point: the binding has no backend to ask, only what the main thread left.
        let rig = Rig::with_screen(ScreenSnapshot {
            backing: 2.0,
            visible_points: (4000, 3000),
        });
        let open = "window.open{title='t', unit='pixels', width=1281, height=801, resizable=true, \
                    min_width=1280, min_height=800, clear={0,0,0}}";
        assert_eq!(error_of(&rig, open), "ok");
        let desc = rig.requests.reader().take().open.expect("the open request");
        assert_eq!(desc.unit, SizeUnit::Points, "the slot holds points");
        assert_eq!((desc.width, desc.height), (641, 401), "rounded up");
        assert_eq!(desc.min, Some((640, 400)));
        assert!(desc.resizable);
        assert_eq!(
            desc.pixel_request,
            Some(ck_window::PixelRequest {
                size: (1281, 801),
                min: Some((1280, 800)),
            })
        );
    }

    #[test]
    fn a_run_with_no_screen_resolves_pixels_at_one_to_one_and_a_points_open_has_no_pixel_request() {
        let rig = Rig::new(false);
        assert_eq!(
            error_of(
                &rig,
                "window.open{title='t', unit='pixels', width=1280, height=800, resizable=true, \
                 min_width=1280, min_height=800, clear={0,0,0}}"
            ),
            "ok"
        );
        let desc = rig.requests.reader().take().open.expect("the open request");
        assert_eq!(
            (desc.width, desc.height, desc.min),
            (1280, 800, Some((1280, 800)))
        );
        let points = Rig::new(false);
        assert_eq!(error_of(&points, OPEN), "ok");
        let desc = points
            .requests
            .reader()
            .take()
            .open
            .expect("the open request");
        assert_eq!(desc.pixel_request, None);
        assert!(!desc.resizable && desc.min.is_none());
    }

    #[test]
    fn the_new_fields_are_refused_with_the_field_named_and_nothing_recorded() {
        let rig = Rig::new(false);
        let base = "title='t', clear={0,0,0}";
        for (fields, wanted) in [
            ("width=800, height=600, unit='furlongs'", "`unit` must be"),
            ("width=800, height=600, unit=3", "`unit` must be"),
            (
                "width=800, height=600, resizable='yes'",
                "`resizable` must be",
            ),
            (
                "width=800, height=600, resizable=true, min_width=100",
                "`min_width` needs `min_height`",
            ),
            (
                "width=800, height=600, resizable=true, min_height=100",
                "`min_height` needs `min_width`",
            ),
            (
                "width=800, height=600, resizable=true, min_width=0, min_height=100",
                "min_width 0 is not from 1 to 16384",
            ),
            (
                "width=800, height=600, resizable=true, min_width=100, min_height=16385",
                "min_height 16385 is not from 1 to 16384",
            ),
            (
                "width=800, height=600, resizable=true, min_width=801, min_height=100",
                "min_width 801 is larger than the opening size 800",
            ),
            (
                "unit='pixels', width=1280, height=800, resizable=true, min_width=100, min_height=801",
                "min_height 801 is larger than the opening size 800",
            ),
            (
                "width=800, height=600, min_width=100, min_height=100",
                "need a resizable window",
            ),
        ] {
            let error = error_of(&rig, &format!("window.open{{{base}, {fields}}}"));
            assert!(error.contains(wanted), "{fields}: {error}");
        }
        let changes = rig.requests.reader().take();
        assert!(changes.open.is_none(), "nothing invalid was recorded");
        // A valid description in pixels with a minimum is accepted.
        assert_eq!(
            error_of(
                &rig,
                "window.open{title='t', clear={0,0,0}, unit='pixels', width=1280, height=800, resizable=true, \
                 min_width=1280, min_height=800}"
            ),
            "ok"
        );
    }

    #[test]
    fn open_returns_the_size_in_points_the_window_opens_at() {
        // 1280x800 pixels at 2 pixels a point open at 640x400 points, which is what the script's first layout starts from.
        let rig = Rig::with_screen(ScreenSnapshot {
            backing: 2.0,
            visible_points: (4000, 3000),
        });
        let values = lua(
            &rig,
            "return window.open{title='t', unit='pixels', width=1280, height=800, resizable=true, \
             min_width=1280, min_height=800, clear={0,0,0}}",
        );
        assert_eq!(
            values,
            [
                Some(ConfigValue::Integer(640)),
                Some(ConfigValue::Integer(400))
            ]
        );
        // A request in points returns it as it is.
        let points = Rig::new(false);
        let values = lua(
            &points,
            "return window.open{title='t', width=200, height=100, clear={0,0,0}}",
        );
        assert_eq!(
            values,
            [
                Some(ConfigValue::Integer(200)),
                Some(ConfigValue::Integer(100))
            ]
        );
    }
}
