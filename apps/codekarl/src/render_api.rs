//! The `render` extension of the UI VM (design §5, §8.2): the Lua surface of the scene bridge.
//!
//! [`extension`] builds the `ck-lua` [`Extension`] the VM named `ui` gets, next to `window`, `texture` and `frame`; no other
//! VM does. Its host functions run on the UI VM's owner thread:
//!
//! - `render.quad{image=, x=, y=, width=, height=}` opens and decodes the named image from the assets directory
//!   ([`Assets`]: a relative name, beneath the root, the root resolved at the first call), builds a [`Quad`]
//!   (`x` and `y` default to 0, `width` and `height` to the image's size in points) and writes it through the
//!   run's one [`SceneWriter`]. Every error is an ordinary Lua error naming the file and the reason, and **a
//!   failed call changes nothing**: the image is opened and the quad built and checked before the writer is
//!   touched;
//! - `render.clear_quad()` clears the quad. On the **upgraded** backend (the flag the composition sets; false until
//!   milestone 5-8c) both legacy calls go through [`LegacyLane`], with its errors prefixed `render.quad:`; on headless runs
//!   and `glfw-vulkan` they keep exactly the path above;
//! - the scene API of the design (§8.2): `render.create_quad{texture=, x=, y=, width=, height=}` → a quad token,
//!   `render.set_position(quad, x, y)`, `render.set_rect(quad, x, y, width, height)` and `render.destroy_quad(quad)`. Each
//!   validates every argument before it changes anything, and reports a refusal as an ordinary Lua error starting with the
//!   function's name. They edit the writer's scene; the frame is published by the UI VM's completion hook
//!   ([`RenderEnd::completion_hook`]) after startup and after every posted callback, at most one frame each, so no call
//!   copies the scene.
//!
//! The writer is `Send` but not `Sync`, while host functions must be `Send + Sync`, so it sits in a
//! `Mutex<Option<SceneWriter>>`. The lock is **owner-only and uncontended**: during the run only the UI VM's owner thread
//! takes it (host calls and the completion hook run there, one at a time) and the composition takes it once, after that
//! thread has finished, for [`RenderEnd::end`], which moves the writer out and drops it. It is never held across a call into
//! Lua, an upload, a wait or I/O, and no other thread takes it.
//!
//! **After the run's end.** The new scene calls validate their cheap arguments and then raise the service's `Ended` error,
//! and retain nothing (no edit, no `TextureRef`, no publication): once the resource service was closed (admission closes
//! before the VMs' `shutdown()`), and once the writer's `end()` emptied the slot. The check reads the service's closed state
//! first; a close that lands just after it is caught by the writer's slot at the latest (the next call, or `end`, reports
//! `Ended`). Only the legacy calls keep M4's semantics after the end: discarded and counted.
//!
//! **`glfw-vulkan`** has no resource service: each new call validates its arguments and then raises "`<function>` is not
//! available on the glfw-vulkan backend until milestone 5.5" (nothing is silently drawn as one of several quads).
//!
//! **Headless runs** get the same extension: every call is validated exactly as in a windowed run and the result is
//! applied to the editable scene, whose publications the bridge stores and nothing draws. The first accepted scene call
//! logs one `info render: scene calls ignored: headless run` record and every later one is counted ([`HeadlessNotes`]) in
//! the run's one `debug` record with the window calls. The scene writer is used in both kinds of run, so that once the
//! application ends the bridge, a late write (the UI VM's `shutdown()`, a decode still running) is discarded and counted the
//! same way.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ck_lua::{Completion, ConfigValue, Extension, HostFunction};
use ck_render_2d::{Assets, LegacyError, LegacyLane, Quad, QuadKey, SceneWriter, validate_rect};
use ck_resources::{AdmitError, ResourceService, TextureKey};

use crate::texture_api::unavailable;
use crate::window_api::{HeadlessNotes, number};

/// What the host functions share.
struct Shared {
    /// The assets rule over the configured directory; `None` without a configuration file.
    assets: Option<Assets>,
    /// The run's one scene writer.
    writer: Mutex<Option<SceneWriter>>,
    /// The headless notes (a headless run).
    headless: Option<Arc<HeadlessNotes>>,
    /// The run's resource service; `None` on `glfw-vulkan`, where the new calls are not available.
    service: Option<ResourceService>,
    /// The legacy lane over the service and the assets root; `None` without a service.
    lane: Option<LegacyLane>,
    /// Whether the legacy calls go through the lane (the upgraded backend).
    upgraded: bool,
    /// Runs after the image is decoded and the quad built, before the scene is written: a test holds a decode
    /// "in flight" here.
    #[cfg(test)]
    after_decode: Option<Box<dyn Fn() + Send + Sync>>,
    /// The threads that took the writer's lock (a test counts them per thread).
    #[cfg(test)]
    lock_threads: Mutex<Vec<std::thread::ThreadId>>,
    /// Called by the upgraded `render.quad` right before and right after the lane's admission (the synchronous read and
    /// decode), with the writer's lock free: a test looks at the lock there.
    #[cfg(test)]
    lane_seam: Mutex<Option<LaneSeam>>,
}

/// A test's look at the upgraded `render.quad`, called with the name of the point it is at.
#[cfg(test)]
type LaneSeam = Arc<dyn Fn(&'static str) + Send + Sync>;

/// The keys `render.quad` takes.
const FIELDS: [&str; 5] = ["image", "x", "y", "width", "height"];
/// The keys `render.create_quad` takes.
const QUAD_FIELDS: [&str; 5] = ["texture", "x", "y", "width", "height"];

/// A number field: absent (`None`), or a number; anything else is an error naming the field.
fn field(
    function: &str,
    table: &BTreeMap<String, ConfigValue>,
    name: &str,
) -> Result<Option<f64>, String> {
    match table.get(name) {
        None => Ok(None),
        Some(value) => number(value)
            .map(Some)
            .ok_or_else(|| format!("{function}: `{name}` must be a number")),
    }
}

/// A number argument at `position` of `function`'s arguments.
fn number_argument(
    function: &str,
    arguments: &[Option<ConfigValue>],
    position: usize,
    name: &str,
) -> Result<f64, String> {
    arguments
        .get(position)
        .and_then(Option::as_ref)
        .and_then(number)
        .ok_or_else(|| format!("{function}: `{name}` must be a number"))
}

/// A quad token argument (the first one): a string, parsed strictly.
fn quad_argument(function: &str, arguments: &[Option<ConfigValue>]) -> Result<QuadKey, String> {
    let Some(Some(ConfigValue::String(text))) = arguments.first() else {
        return Err(format!(
            "{function}: expected a quad token as the first argument"
        ));
    };
    QuadKey::from_token(text).map_err(|error| format!("{function}: {error}"))
}

/// The exact number of arguments, or the error.
fn arity(
    function: &str,
    arguments: &[Option<ConfigValue>],
    wanted: usize,
    usage: &str,
) -> Result<(), String> {
    if arguments.len() == wanted {
        Ok(())
    } else {
        Err(format!("{function}: expected {usage}"))
    }
}

impl Shared {
    /// The writer's lock. A test counts the threads that take it.
    fn writer(&self) -> MutexGuard<'_, Option<SceneWriter>> {
        #[cfg(test)]
        self.lock_threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(std::thread::current().id());
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn note_accepted(&self) {
        if let Some(notes) = &self.headless {
            notes.accepted_render();
        }
    }

    /// The service of a new scene call, after its arguments were checked: `glfw-vulkan` has none, and after the run's end the
    /// service is closed. Nothing is retained or edited on either refusal.
    fn scene_service(&self, function: &str) -> Result<&ResourceService, String> {
        let Some(service) = &self.service else {
            return Err(unavailable(function));
        };
        if service.is_closed() {
            return Err(format!("{function}: {}", AdmitError::Ended));
        }
        Ok(service)
    }

    fn quad(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let [Some(ConfigValue::Table(table))] = arguments else {
            return Err("render.quad: expected one table {image=, x=, y=, width=, height=}".into());
        };
        if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(format!("render.quad: unknown field `{unknown}`"));
        }
        let name = match table.get("image") {
            Some(ConfigValue::String(name)) => name,
            Some(_) => return Err("render.quad: `image` must be a string".into()),
            None => return Err("render.quad: `image` is missing".into()),
        };
        let (x, y) = (
            field("render.quad", table, "x")?.unwrap_or(0.0),
            field("render.quad", table, "y")?.unwrap_or(0.0),
        );
        let (width, height) = (
            field("render.quad", table, "width")?,
            field("render.quad", table, "height")?,
        );
        if self.upgraded {
            return self.quad_through_the_lane(name, x, y, width, height);
        }
        let Some(assets) = &self.assets else {
            return Err(
                "render.quad: there is no assets directory: run with --config, so that the `[render]` table's `assets` has a directory to be relative to"
                    .into(),
            );
        };
        let image = assets
            .open(name)
            .map_err(|error| format!("render.quad: {error}"))?;
        let quad = Quad::new(Arc::new(image), x, y, width, height)
            .map_err(|error| format!("render.quad: {name}: {error}"))?;
        #[cfg(test)]
        if let Some(hold) = &self.after_decode {
            hold();
        }
        // Everything was checked: now, and only now, the scene changes.
        let writer = self.writer();
        let Some(writer) = writer.as_ref() else {
            return Err("render: run has ended".into());
        };
        writer.set_quad(quad);
        self.note_accepted();
        Ok(Vec::new())
    }

    /// `render.quad` on the upgraded backend: the lane reads and decodes the image and admits the texture **without the
    /// writer's lock** (that is synchronous I/O), and the lock is taken only for the O(1) looks and to install.
    fn quad_through_the_lane(
        &self,
        name: &str,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<Vec<ConfigValue>, String> {
        let Some(lane) = &self.lane else {
            return Err("render.quad: there is no resource service on this backend".into());
        };
        let ended = match self.writer().as_ref() {
            Some(writer) => writer.is_ended(),
            None => return Err("render: run has ended".into()),
        };
        #[cfg(test)]
        self.seam("before_admit");
        let admission = lane
            .admit(ended, name, x, y, width, height)
            .map_err(|error| legacy_message(&error, name))?;
        #[cfg(test)]
        self.seam("after_admit");
        // A writer that ended while the image decoded drops the admission, which releases its texture.
        let writer = self.writer();
        let Some(writer) = writer.as_ref() else {
            return Err("render: run has ended".into());
        };
        lane.install(writer, admission)
            .map_err(|error| legacy_message(&error, name))?;
        self.note_accepted();
        Ok(Vec::new())
    }

    #[cfg(test)]
    fn seam(&self, point: &'static str) {
        let seam = self
            .lane_seam
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(seam) = seam {
            seam(point);
        }
    }

    fn clear_quad(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        if !arguments.is_empty() {
            return Err("render.clear_quad takes no arguments".into());
        }
        let writer = self.writer();
        let Some(writer) = writer.as_ref() else {
            return Err("render: run has ended".into());
        };
        match (&self.lane, self.upgraded) {
            (Some(lane), true) => lane.clear(writer),
            _ => writer.clear_quad(),
        }
        self.note_accepted();
        Ok(Vec::new())
    }

    fn create_quad(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let [Some(ConfigValue::Table(table))] = arguments else {
            return Err(
                "render.create_quad: expected one table {texture=, x=, y=, width=, height=}".into(),
            );
        };
        if let Some(unknown) = table
            .keys()
            .find(|key| !QUAD_FIELDS.contains(&key.as_str()))
        {
            return Err(format!("render.create_quad: unknown field `{unknown}`"));
        }
        let texture = match table.get("texture") {
            Some(ConfigValue::String(text)) => text,
            Some(_) => {
                return Err("render.create_quad: `texture` must be a texture token string".into());
            }
            None => return Err("render.create_quad: `texture` is missing".into()),
        };
        let key = TextureKey::from_token(texture)
            .map_err(|error| format!("render.create_quad: {error}"))?;
        let x = field("render.create_quad", table, "x")?.unwrap_or(0.0);
        let y = field("render.create_quad", table, "y")?.unwrap_or(0.0);
        let width = field("render.create_quad", table, "width")?
            .ok_or("render.create_quad: `width` is missing")?;
        let height = field("render.create_quad", table, "height")?
            .ok_or("render.create_quad: `height` is missing")?;
        // Every cheap check comes before the backend/ended refusal and before any reference is taken.
        validate_rect(x, y, width, height)
            .map_err(|error| format!("render.create_quad: {error}"))?;
        let service = self.scene_service("render.create_quad")?;
        let mut writer = self.writer();
        let Some(writer) = writer.as_mut() else {
            return Err(format!("render.create_quad: {}", AdmitError::Ended));
        };
        // The reference is taken only now, after every check, and is dropped again by the writer's own refusal (geometry,
        // a full scene): nothing stays retained by a call that fails.
        let reference = service
            .retain(key)
            .map_err(|error| format!("render.create_quad: {error}"))?;
        let quad = writer
            .create_quad(reference, x, y, width, height)
            .map_err(|error| format!("render.create_quad: {error}"))?;
        self.note_accepted();
        Ok(vec![ConfigValue::String(quad.token().to_string())])
    }

    fn set_position(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        const F: &str = "render.set_position";
        arity(F, arguments, 3, "(quad, x, y)")?;
        let quad = quad_argument(F, arguments)?;
        let (x, y) = (
            number_argument(F, arguments, 1, "x")?,
            number_argument(F, arguments, 2, "y")?,
        );
        // The writer's own rule for a position (a one-point rectangle), before the backend/ended refusal.
        validate_rect(x, y, 1.0, 1.0).map_err(|error| format!("{F}: {error}"))?;
        self.scene_service(F)?;
        let mut writer = self.writer();
        let Some(writer) = writer.as_mut() else {
            return Err(format!("{F}: {}", AdmitError::Ended));
        };
        writer
            .set_position(quad, x, y)
            .map_err(|error| format!("{F}: {error}"))?;
        self.note_accepted();
        Ok(Vec::new())
    }

    fn set_rect(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        const F: &str = "render.set_rect";
        arity(F, arguments, 5, "(quad, x, y, width, height)")?;
        let quad = quad_argument(F, arguments)?;
        let (x, y, width, height) = (
            number_argument(F, arguments, 1, "x")?,
            number_argument(F, arguments, 2, "y")?,
            number_argument(F, arguments, 3, "width")?,
            number_argument(F, arguments, 4, "height")?,
        );
        validate_rect(x, y, width, height).map_err(|error| format!("{F}: {error}"))?;
        self.scene_service(F)?;
        let mut writer = self.writer();
        let Some(writer) = writer.as_mut() else {
            return Err(format!("{F}: {}", AdmitError::Ended));
        };
        writer
            .set_rect(quad, x, y, width, height)
            .map_err(|error| format!("{F}: {error}"))?;
        self.note_accepted();
        Ok(Vec::new())
    }

    fn destroy_quad(&self, arguments: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        const F: &str = "render.destroy_quad";
        arity(F, arguments, 1, "(quad)")?;
        let quad = quad_argument(F, arguments)?;
        self.scene_service(F)?;
        let mut writer = self.writer();
        let Some(writer) = writer.as_mut() else {
            return Err(format!("{F}: {}", AdmitError::Ended));
        };
        writer
            .destroy_quad(quad)
            .map_err(|error| format!("{F}: {error}"))?;
        self.note_accepted();
        Ok(Vec::new())
    }
}

/// The text of a legacy-lane refusal: the old wording with the `render.quad:` prefix.
fn legacy_message(error: &LegacyError, name: &str) -> String {
    match error {
        LegacyError::Geometry(error) => format!("render.quad: {name}: {error}"),
        other => format!("render.quad: {other}"),
    }
}

/// The `render` extension. `assets` is the configured assets directory (`None` without a configuration
/// file), `writer` the run's scene writer and `headless` the notes of a headless run. It has no resource
/// service: the new scene calls are not available (as on `glfw-vulkan`) and the legacy calls keep their old path.
#[cfg(test)]
pub(crate) fn extension(
    assets: Option<PathBuf>,
    writer: SceneWriter,
    headless: Option<Arc<HeadlessNotes>>,
) -> Extension {
    extension_with_end(assets, writer, headless, None, false).0
}

/// An explicit end for the shared writer, which may otherwise be kept alive by the runtime's
/// parent wiring and owner spawn closures after the VM threads have stopped.
#[derive(Clone)]
pub(crate) struct RenderEnd(Arc<Shared>);

impl RenderEnd {
    /// Ends the writer: it is moved out of its slot and dropped, so every later scene call raises `Ended` and holds
    /// nothing. Taken once, by the composition, after the UI VM's thread has finished.
    pub(crate) fn end(&self) {
        let writer = self.0.writer().take();
        drop(writer);
    }

    /// The completion hook of the UI VM (design §8.1): after `startup()` and after every posted callback that has a result,
    /// on the owner thread, it publishes the scene (a frame only when something changed; none after the writer ended).
    pub(crate) fn completion_hook(&self) -> Arc<dyn Fn(Completion) + Send + Sync> {
        let shared = Arc::clone(&self.0);
        Arc::new(move |_| {
            if let Some(writer) = shared.writer().as_mut() {
                writer.publish();
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn add_test_quad(
        &self,
        texture: ck_resources::TextureRef<ck_resources::ResourceState>,
    ) {
        let mut writer = self.0.writer();
        writer
            .as_mut()
            .expect("test scene writer is still active")
            .create_quad(texture, 0.0, 0.0, 1.0, 1.0)
            .expect("test quad fits the configured scene");
    }

    /// Installs the seam of the upgraded `render.quad` (see `Shared::lane_seam`).
    #[cfg(test)]
    pub(crate) fn set_lane_seam(&self, seam: LaneSeam) {
        *self
            .0
            .lane_seam
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(seam);
    }

    /// Whether the writer's lock is free right now (a `try_lock` that lets go at once).
    #[cfg(test)]
    pub(crate) fn writer_lock_is_free(&self) -> bool {
        self.0.writer.try_lock().is_ok()
    }

    /// The threads that took the writer's lock so far, in order (a test counts them per thread).
    #[cfg(test)]
    pub(crate) fn lock_threads(&self) -> Vec<std::thread::ThreadId> {
        self.0
            .lock_threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The extension and its end. `service` is the run's resource service (`None` on `glfw-vulkan`); `upgraded` says the
/// legacy calls go through the lane (the composition passes `false` until milestone 5-8c makes native-metal upgraded).
pub(crate) fn extension_with_end(
    assets: Option<PathBuf>,
    writer: SceneWriter,
    headless: Option<Arc<HeadlessNotes>>,
    service: Option<ResourceService>,
    upgraded: bool,
) -> (Extension, RenderEnd) {
    let lane = service
        .clone()
        .map(|service| LegacyLane::new(service, assets.clone().map(Assets::new)));
    let shared = Arc::new(Shared {
        assets: assets.map(Assets::new),
        writer: Mutex::new(Some(writer)),
        headless,
        service,
        lane,
        upgraded,
        #[cfg(test)]
        after_decode: None,
        #[cfg(test)]
        lock_threads: Mutex::new(Vec::new()),
        #[cfg(test)]
        lane_seam: Mutex::new(None),
    });
    (build(&shared), RenderEnd(shared))
}

/// [`extension`], with `hold` run between the decode and the write of every `render.quad`.
#[cfg(test)]
pub(crate) fn extension_holding(
    assets: Option<PathBuf>,
    writer: SceneWriter,
    hold: Box<dyn Fn() + Send + Sync>,
) -> Extension {
    build(&Arc::new(Shared {
        assets: assets.map(Assets::new),
        writer: Mutex::new(Some(writer)),
        headless: None,
        service: None,
        lane: None,
        upgraded: false,
        after_decode: Some(hold),
        lock_threads: Mutex::new(Vec::new()),
        lane_seam: Mutex::new(None),
    }))
}

/// A host function of this module: a method of [`Shared`].
type HostCall = fn(&Shared, &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String>;

fn build(shared: &Arc<Shared>) -> Extension {
    let function = |call: HostCall| {
        let shared = Arc::clone(shared);
        let function: HostFunction = Arc::new(move |arguments| call(&shared, arguments));
        function
    };
    Extension::new("render")
        .function("quad", function(Shared::quad))
        .function("clear_quad", function(Shared::clear_quad))
        .function("create_quad", function(Shared::create_quad))
        .function("set_position", function(Shared::set_position))
        .function("set_rect", function(Shared::set_rect))
        .function("destroy_quad", function(Shared::destroy_quad))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    use ck_log::Level;
    use ck_render_2d::{Filter, QuadSink, SceneBridge, Snapshot};
    use ck_window::{PixelSize, Requests, WindowDesc};

    use super::*;
    use crate::testing::{Dir, error_in_ui, png, run_in_ui};
    use crate::window_api;

    /// A sink that keeps every snapshot.
    #[derive(Default)]
    struct Seen(Mutex<Vec<Snapshot>>);

    impl QuadSink for Seen {
        fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(snapshot);
            None
        }
    }

    impl Seen {
        fn all(&self) -> Vec<Snapshot> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn last(&self) -> Snapshot {
            self.all().pop().expect("a snapshot")
        }
    }

    /// An assets directory with a few images, a windowed bridge attached to a recording sink, and the `window`
    /// and `render` extensions over them.
    struct Rig {
        _assets: Dir,
        seen: Arc<Seen>,
        bridge: Arc<SceneBridge>,
        extensions: Vec<Extension>,
    }

    fn solid(width: u32, height: u32) -> Vec<u8> {
        png(
            width,
            height,
            &vec![200; width as usize * height as usize * 4],
        )
    }

    impl Rig {
        fn with_assets(path: Option<&Path>, dir: Dir) -> Self {
            let bridge = SceneBridge::new(Filter::Nearest);
            let seen = Arc::new(Seen::default());
            bridge
                .attach(
                    &WindowDesc {
                        title: "t".to_owned(),
                        width: 200,
                        height: 100,
                        clear: [0.0; 3],
                    },
                    PixelSize {
                        width: 200,
                        height: 100,
                    },
                    Arc::clone(&seen) as Arc<dyn QuadSink>,
                )
                .expect("attached");
            let writer = bridge.take_writer().expect("the one writer");
            let requests = Requests::new();
            let window = window_api::extension(requests.writer(ck_window::Notifier::new()), None);
            let render = extension(path.map(Path::to_path_buf), writer, None);
            Self {
                _assets: dir,
                seen,
                bridge,
                extensions: vec![window, render],
            }
        }

        fn new() -> Self {
            let dir = Dir::new();
            fs::write(dir.0.join("small.png"), solid(8, 4)).expect("an image");
            fs::write(dir.0.join("notpng.png"), b"this is not a PNG file").expect("a file");
            // A header that claims an image over the dimension cap (8193 wide), and nothing else.
            let mut oversized = solid(1, 1);
            oversized[16..20].copy_from_slice(&8193_u32.to_be_bytes());
            fs::write(dir.0.join("huge.png"), oversized).expect("a file");
            fs::create_dir_all(dir.0.join("sub")).expect("a directory");
            fs::write(dir.0.join("sub/inner.png"), solid(2, 2)).expect("an image");
            let path = dir.0.clone();
            Self::with_assets(Some(&path), dir)
        }

        fn error(&self, call: &str) -> String {
            error_in_ui(&self.extensions, call)
        }

        fn generation(&self) -> u64 {
            self.seen.last().generation
        }
    }

    #[test]
    fn a_quad_defaults_to_the_origin_and_the_images_size_and_takes_what_it_is_given() {
        let rig = Rig::new();
        assert_eq!(rig.error("render.quad{image='small.png'}"), "ok");
        let placed = rig.seen.last().quad.expect("a quad");
        assert_eq!(
            (placed.destination.left, placed.destination.top),
            (0, 0),
            "x and y default to 0"
        );
        assert_eq!(
            (placed.destination.right, placed.destination.bottom),
            (8, 4),
            "the size defaults to the image's, one pixel per point"
        );
        assert_eq!(
            rig.error("render.quad{image='sub/inner.png', x=10, y=20.5, width=40, height=30}"),
            "ok"
        );
        let placed = rig.seen.last().quad.expect("a quad");
        assert_eq!(placed.destination.left, 10);
        assert_eq!(placed.destination.right, 50);
        assert_eq!(placed.destination.bottom - placed.destination.top, 30);
    }

    #[test]
    fn clear_quad_clears_and_is_harmless_without_a_quad() {
        let rig = Rig::new();
        assert_eq!(rig.error("render.clear_quad()"), "ok", "no quad yet");
        assert!(rig.seen.last().quad.is_none());
        assert_eq!(rig.error("render.quad{image='small.png'}"), "ok");
        assert!(rig.seen.last().quad.is_some());
        assert_eq!(rig.error("render.clear_quad()"), "ok");
        assert!(rig.seen.last().quad.is_none());
        assert!(!rig.seen.last().requested, "cleared means not requested");
        assert!(
            rig.error("render.clear_quad(1)")
                .contains("takes no arguments")
        );
    }

    #[test]
    fn a_failed_call_raises_a_lua_error_naming_the_file_and_leaves_the_previous_quad_unchanged() {
        let rig = Rig::new();
        assert_eq!(rig.error("render.quad{image='small.png', x=5}"), "ok");
        let before = (rig.generation(), rig.seen.all().len());
        for (call, wanted) in [
            ("render.quad{image='/etc/passwd'}", "/etc/passwd"),
            ("render.quad{image='../small.png'}", "../small.png"),
            ("render.quad{image='missing.png'}", "missing.png"),
            ("render.quad{image='notpng.png'}", "notpng.png"),
            ("render.quad{image='huge.png'}", "huge"),
            ("render.quad{image='small.png', x=1/0}", "finite"),
            ("render.quad{image='small.png', width=0/0}", "finite"),
            ("render.quad{image='small.png', width=0}", "width"),
            (
                "render.quad{image='small.png', x='1'}",
                "`x` must be a number",
            ),
            ("render.quad{image=3}", "`image` must be a string"),
            ("render.quad{}", "`image` is missing"),
            (
                "render.quad{image='small.png', extra=1}",
                "unknown field `extra`",
            ),
            ("render.quad()", "expected one table"),
            ("render.quad(3)", "expected one table"),
        ] {
            let error = rig.error(call);
            assert!(error.contains("render.quad"), "{call}: {error}");
            assert!(error.contains(wanted), "{call}: {error}");
        }
        assert_eq!(
            (rig.generation(), rig.seen.all().len()),
            before,
            "no failed call changed or published anything"
        );
        assert_eq!(rig.bridge.counts().accepted, 1, "only the good write");
    }

    #[test]
    fn with_no_assets_directory_the_first_and_every_later_call_say_so() {
        let rig = Rig::with_assets(None, Dir::new());
        for _ in 0..2 {
            let error = rig.error("render.quad{image='small.png'}");
            assert!(error.contains("no assets directory"), "{error}");
            assert!(error.contains("--config"), "{error}");
        }
        assert_eq!(rig.bridge.counts().accepted, 0);
    }

    #[test]
    fn a_missing_assets_root_is_an_error_on_the_first_call_and_on_a_later_one() {
        let parent = Dir::new();
        let missing = parent.0.join("no-such-assets");
        let rig = Rig::with_assets(Some(&missing), Dir::new());
        for _ in 0..2 {
            let error = rig.error("render.quad{image='small.png'}");
            assert!(error.contains("no-such-assets"), "{error}");
        }
        // The root is resolved at the first call, not before: creating it afterwards does not rescue the
        // kept error, and a root that exists from the start works.
        assert_eq!(rig.bridge.counts().accepted, 0);
    }

    #[test]
    fn render_beside_window_both_work_in_one_script() {
        let rig = Rig::new();
        let values = run_in_ui(
            &rig.extensions,
            "window.open{title='t', width=200, height=100, clear={0,0,0}}\n\
             render.quad{image='small.png'}\n\
             return type(render.quad), type(render.clear_quad), type(window.close)",
        );
        let kinds: Vec<_> = values
            .iter()
            .map(|value| match value {
                Some(ConfigValue::String(text)) => text.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, ["function", "function", "function"]);
        assert!(rig.seen.last().quad.is_some());
    }

    /// The records a headless run's notes wrote: level, category and text.
    type Records = Arc<Mutex<Vec<(Level, &'static str, String)>>>;

    /// The headless notes and the records they wrote.
    fn headless() -> (Arc<HeadlessNotes>, Records) {
        let records = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&records);
        let notes = Arc::new(HeadlessNotes::with_sink(Box::new(
            move |level, category, text| {
                kept.lock().unwrap_or_else(PoisonError::into_inner).push((
                    level,
                    category,
                    text.to_owned(),
                ));
            },
        )));
        (notes, records)
    }

    #[test]
    fn a_headless_run_validates_and_decodes_then_ignores_with_one_record_and_counts_the_rest() {
        let dir = Dir::new();
        fs::write(dir.0.join("small.png"), solid(8, 4)).expect("an image");
        let (notes, records) = headless();
        let path = dir.0.clone();
        let requests = Requests::new();
        let writer = SceneBridge::new(Filter::Nearest)
            .take_writer()
            .expect("the one writer");
        let extensions = vec![
            window_api::extension(
                requests.writer(ck_window::Notifier::new()),
                Some(Arc::clone(&notes)),
            ),
            extension(Some(path), writer, Some(Arc::clone(&notes))),
        ];
        // Validation and decoding are the windowed run's: a bad call is the same error, and logs nothing.
        assert!(
            error_in_ui(&extensions, "render.quad{image='missing.png'}").contains("missing.png")
        );
        assert!(
            error_in_ui(&extensions, "render.quad{image='small.png', x=1/0}").contains("finite")
        );
        assert!(
            records.lock().unwrap().is_empty(),
            "rejected calls log nothing"
        );
        assert_eq!(
            error_in_ui(&extensions, "render.quad{image='small.png'}"),
            "ok"
        );
        assert_eq!(
            *records.lock().unwrap(),
            [(
                Level::Info,
                "render",
                "scene calls ignored: headless run".to_owned()
            )],
            "the first accepted call logs one record"
        );
        for call in [
            "render.quad{image='small.png', x=3}",
            "render.clear_quad()",
            "render.clear_quad()",
        ] {
            assert_eq!(error_in_ui(&extensions, call), "ok");
        }
        assert_eq!(
            records.lock().unwrap().len(),
            1,
            "later calls are counted, not logged"
        );
        assert_eq!(
            notes.final_count().as_deref(),
            Some("3 further render request(s) ignored: headless run")
        );
        assert_eq!(error_in_ui(&extensions, "window.close()"), "ok");
        assert_eq!(
            notes.final_count().as_deref(),
            Some(
                "1 further window request(s) and 3 further render request(s) ignored: headless run"
            ),
            "counted together with the window calls"
        );
    }

    #[test]
    fn the_same_script_gives_the_same_lua_results_headless_and_windowed() {
        let windowed = Rig::new();
        let dir = Dir::new();
        fs::write(dir.0.join("small.png"), solid(8, 4)).expect("an image");
        let (notes, _records) = headless();
        let path = dir.0.clone();
        let requests = Requests::new();
        let writer = SceneBridge::new(Filter::Nearest)
            .take_writer()
            .expect("the one writer");
        let headless_extensions = vec![
            window_api::extension(
                requests.writer(ck_window::Notifier::new()),
                Some(Arc::clone(&notes)),
            ),
            extension(Some(path), writer, Some(notes)),
        ];
        for call in [
            "render.quad{image='small.png'}",
            "render.quad{image='small.png', width=-1}",
            "render.quad{image='nope.png'}",
            "render.quad{image='../small.png'}",
            "render.quad{image='small.png', y=0/0}",
            "render.quad{}",
            "render.clear_quad()",
            "render.clear_quad(1)",
        ] {
            let (left, right) = (
                windowed.error(call),
                error_in_ui(&headless_extensions, call),
            );
            // The error text names the scratch directory in the file errors; compare the parts that do not.
            assert_eq!(
                left.contains("ok") && left == "ok",
                right == "ok",
                "{call}: {left} / {right}"
            );
        }
    }
}
