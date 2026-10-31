//! CK-125: the Lua texture, quad and frame API of the UI VM, over real UI VMs without a window.
//!
//! A rig composes what the application composes for a mode: a bridge with a recording sink, a resource service (stepped by
//! the test as the backend and the loaders), the `render`, `texture` and `frame` extensions, and the completion hook that
//! publishes the scene. Each test names its negative control.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ck_lua::{CallOutcome, ConfigValue, Extension, VmOwner, VmSettings};
use ck_render_2d::{Filter, QuadSink, SceneBridge, Snapshot};
use ck_resources::{
    Assets, CommitOutcome, Mode, ResourceLimits, ResourceService, ServiceWakes, TextureKey,
};
use ck_window::{Notifier, PixelSize, Requests, UpdateSettings, WindowDesc};

use crate::frame_api::{self, FrameBackend};
use crate::render_api::{self, RenderEnd};
use crate::testing::{Dir, error_in_ui, png, run_in_ui};
use crate::texture_api;
use crate::window_api::{self, HeadlessNotes};

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
    fn count(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    fn last(&self) -> Snapshot {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .cloned()
            .expect("a snapshot")
    }

    /// The compatibility quad of the last snapshot's scene frame as (x, y, width, height).
    fn compat(&self) -> Option<[f64; 4]> {
        self.last().scene.and_then(|frame| {
            frame
                .compat()
                .map(|quad| [quad.x, quad.y, quad.width, quad.height])
        })
    }

    /// The scene frame's quads of the last snapshot as (x, y, width, height), in painter order.
    fn rects(&self) -> Vec<[f64; 4]> {
        self.last().scene.map_or_else(Vec::new, |frame| {
            frame
                .quads()
                .iter()
                .map(|quad| [quad.x, quad.y, quad.width, quad.height])
                .collect()
        })
    }
}

/// The backend a rig is composed for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A resource service in backend mode and the `[frame]` update control.
    Native,
    /// A headless service and no update source.
    Headless,
    /// A backend-mode service and the update control, composed for `glfw-vulkan` (the same as `Native` here: the extensions
    /// do not differ between the two windowed backends).
    Glfw,
    /// No service, as a test builds a run without one.
    NoService,
}

type Records = Arc<Mutex<Vec<(ck_log::Level, &'static str, String)>>>;

struct Rig {
    _dir: Dir,
    root: PathBuf,
    service: Option<ResourceService>,
    bridge: Arc<SceneBridge>,
    seen: Arc<Seen>,
    end: RenderEnd,
    extensions: Vec<Extension>,
    control: ck_window::UpdateControl,
    notes: Arc<HeadlessNotes>,
    records: Records,
}

fn wakes() -> ServiceWakes {
    ServiceWakes {
        jobs: None,
        uploads: None,
        retirements: None,
    }
}

impl Rig {
    fn new(kind: Kind) -> Self {
        Self::with(kind, false, ResourceLimits::DEFAULT)
    }

    fn with(kind: Kind, upgraded: bool, limits: ResourceLimits) -> Self {
        let dir = Dir::new();
        fs::write(dir.0.join("a.png"), png(8, 4, &[200; 8 * 4 * 4])).expect("an image");
        fs::write(dir.0.join("b.png"), png(4, 4, &[100; 4 * 4 * 4])).expect("an image");
        let root = dir.0.clone();
        let service = match kind {
            Kind::NoService => None,
            Kind::Native | Kind::Glfw | Kind::Headless => Some(
                ResourceService::with_assets(
                    limits,
                    if kind == Kind::Headless {
                        Mode::Headless
                    } else {
                        Mode::Backend
                    },
                    wakes(),
                    Some(Assets::new(root.clone())),
                )
                .expect("a service"),
            ),
        };
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
        let records: Records = Arc::new(Mutex::new(Vec::new()));
        let notes = {
            let kept = Arc::clone(&records);
            Arc::new(HeadlessNotes::with_sink(Box::new(
                move |level, category, text| {
                    kept.lock().unwrap_or_else(PoisonError::into_inner).push((
                        level,
                        category,
                        text.to_owned(),
                    ));
                },
            )))
        };
        let headless = (kind == Kind::Headless).then(|| Arc::clone(&notes));
        let (render, end) = render_api::extension_with_end(
            Some(root.clone()),
            writer,
            headless.clone(),
            service.clone(),
            upgraded,
        );
        let control = UpdateSettings::new(60, Duration::from_millis(100))
            .expect("valid pacing")
            .control();
        let frame = frame_api::extension(match kind {
            Kind::Native | Kind::Glfw => FrameBackend::Control(control.clone()),
            Kind::Headless | Kind::NoService => FrameBackend::Headless,
        });
        let window = window_api::extension(Requests::new().writer(Notifier::new()), headless);
        Self {
            _dir: dir,
            root,
            service: service.clone(),
            bridge,
            seen,
            end,
            extensions: vec![window, render, texture_api::extension(service), frame],
            control,
            notes,
            records,
        }
    }

    fn service(&self) -> &ResourceService {
        self.service.as_ref().expect("a service")
    }

    /// The error text of `call`, or `ok`, without the `runtime error: ` that Lua puts before a host function's message.
    fn error(&self, call: &str) -> String {
        let text = error_in_ui(&self.extensions, call);
        text.strip_prefix("runtime error: ")
            .map_or(text.clone(), str::to_owned)
    }

    fn run(&self, body: &str) -> Vec<Option<ConfigValue>> {
        run_in_ui(&self.extensions, body)
    }

    /// Runs the loaders: every queued job, to the end.
    fn run_jobs(&self) {
        while let Some(job) = self.service().take_job() {
            self.service().complete(job.run());
        }
    }

    /// Plays the backend: every revision waiting for upload is committed and adopted.
    fn adopt_all(&self) {
        let mut uploads = Vec::new();
        while self.service().take_uploads(&mut uploads, 16) > 0 {
            for upload in uploads.drain(..) {
                assert_eq!(
                    self.service().try_commit(upload.key, upload.revision),
                    CommitOutcome::Committed
                );
                self.service().adopted(upload.key, upload.revision);
            }
        }
    }

    /// The first string a script returned.
    fn token(&self, body: &str) -> String {
        match self.run(body).into_iter().next() {
            Some(Some(ConfigValue::String(text))) => text,
            other => panic!("not a token: {other:?}"),
        }
    }

    /// A VM named `ui` over the rig's extensions with the completion hook, started, whose script is `script`.
    fn vm(&self, script: &str) -> (VmOwner, Dir) {
        self.vm_with(script, Vec::new())
    }

    /// [`vm`](Self::vm) with more extensions (a test's own host functions).
    fn vm_with(&self, script: &str, extra: Vec<Extension>) -> (VmOwner, Dir) {
        let dir = Dir::new();
        let entry = dir.0.join("main.lua");
        fs::write(&entry, script).expect("a script");
        let mut settings = VmSettings::new("ui", entry, ConfigValue::Table(BTreeMap::new()));
        settings.extensions.clone_from(&self.extensions);
        settings.extensions.extend(extra);
        settings.call_limit = 4;
        settings.on_complete = Some(self.end.completion_hook());
        let mut owner = VmOwner::spawn(settings).expect("the VM spawns");
        owner.wait_ready(Duration::from_secs(5)).expect("ready");
        owner.start(Duration::from_secs(5)).expect("started");
        (owner, dir)
    }
}

/// Posts `callback` and waits for its outcome.
fn call(owner: &mut VmOwner, callback: &str) -> CallOutcome {
    owner.post(callback, Vec::new()).expect("posted");
    let give_up = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = owner.take_result() {
            return result.outcome;
        }
        assert!(Instant::now() < give_up, "the callback never answered");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn string(value: Option<&ConfigValue>) -> &str {
    match value {
        Some(ConfigValue::String(text)) => text,
        other => panic!("not a string: {other:?}"),
    }
}

fn integer(value: Option<&ConfigValue>) -> i64 {
    match value {
        Some(ConfigValue::Integer(number)) => *number,
        other => panic!("not an integer: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// texture
// ---------------------------------------------------------------------------------------------

/// `load` returns a token at once, even for a file that is not there; the failure arrives in `status`. Negative control: a
/// `load` that opened the file at the call would raise instead of returning a token.
#[test]
fn texture_load_returns_a_token_at_once_and_a_missing_file_fails_later_in_status() {
    let rig = Rig::new(Kind::Native);
    let values = rig.run(
        "local t = texture.load('missing.png')\n\
         local s = texture.status(t)\n\
         return t, s.state, s.requested, s.shown == nil, s.committed == nil",
    );
    let key = TextureKey::from_token(string(values[0].as_ref())).expect("a texture token");
    assert_eq!(string(values[1].as_ref()), "pending");
    assert_eq!(integer(values[2].as_ref()), 1);
    assert_eq!(
        values[3],
        Some(ConfigValue::Bool(true)),
        "no shown revision"
    );
    assert_eq!(
        values[4],
        Some(ConfigValue::Bool(true)),
        "no committed revision"
    );
    rig.run_jobs();
    let after = rig.run(&format!(
        "local s = texture.status('{}')\nreturn s.state, s.error, s.error_revision, s.shown == nil",
        string(values[0].as_ref())
    ));
    assert_eq!(string(after[0].as_ref()), "failed");
    assert!(
        string(after[1].as_ref()).contains("missing.png"),
        "the error names the file: {after:?}"
    );
    assert_eq!(integer(after[2].as_ref()), 1);
    assert!(rig.service().status(key).is_ok());
}

/// A real file: pending → uploading → ready natively (the test plays the backend), `cpu_ready` and never `ready` headless;
/// `reload` returns increasing revisions and each is shown in turn.
#[test]
fn a_real_file_goes_pending_uploading_ready_natively_and_cpu_ready_headless_and_reload_counts_up() {
    let rig = Rig::new(Kind::Native);
    let token = rig.token("return texture.load('a.png')");
    let state = |rig: &Rig| {
        rig.run(&format!(
            "local s = texture.status('{token}')\nreturn s.state, s.requested, s.committed, s.shown, s.width, s.height"
        ))
    };
    assert_eq!(string(state(&rig)[0].as_ref()), "pending");
    rig.run_jobs();
    let uploading = state(&rig);
    assert_eq!(string(uploading[0].as_ref()), "uploading");
    assert_eq!(uploading[3], None, "nothing is shown before adoption");
    rig.adopt_all();
    let ready = state(&rig);
    assert_eq!(string(ready[0].as_ref()), "ready");
    assert_eq!(
        (
            integer(ready[1].as_ref()),
            integer(ready[2].as_ref()),
            integer(ready[3].as_ref())
        ),
        (1, 1, 1)
    );
    assert_eq!(
        (integer(ready[4].as_ref()), integer(ready[5].as_ref())),
        (8, 4)
    );
    for wanted in [2, 3] {
        let revision = rig.run(&format!("return texture.reload('{token}')"));
        assert_eq!(
            integer(revision[0].as_ref()),
            wanted,
            "increasing revisions"
        );
        rig.run_jobs();
        rig.adopt_all();
        assert_eq!(
            integer(state(&rig)[3].as_ref()),
            wanted,
            "the revision is shown in turn"
        );
    }

    let headless = Rig::new(Kind::Headless);
    let token = headless.token("return texture.load('a.png')");
    headless.run_jobs();
    let states = headless.run(&format!(
        "local s = texture.status('{token}')\nreturn s.state"
    ));
    assert_eq!(
        string(states[0].as_ref()),
        "cpu_ready",
        "headless never reaches ready"
    );
}

/// A released token is stale for every use; a second release is an error; none of it changes the service.
#[test]
fn release_makes_every_use_stale_and_a_second_release_is_an_error() {
    let rig = Rig::new(Kind::Native);
    let token = rig.token("return texture.load('a.png')");
    assert_eq!(rig.error(&format!("texture.release('{token}')")), "ok");
    for call in [
        format!("texture.release('{token}')"),
        format!("texture.status('{token}')"),
        format!("texture.reload('{token}')"),
        format!("render.create_quad{{texture='{token}', width=1, height=1}}"),
    ] {
        let error = rig.error(&call);
        assert_ne!(error, "ok", "{call}");
        let name: String = call
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'))
            .collect();
        assert!(error.starts_with(&format!("{name}:")), "{call}: {error}");
        assert!(error.contains("no longer usable"), "{call}: {error}");
    }
}

/// Every argument error is an ordinary catchable error starting with the function's name, and none changes anything: the
/// registry, the job slots and the scene stay as they were. Negative control: a validation that ran after the admission
/// would leave a registered resource behind (`registry.live` would grow).
#[test]
fn argument_errors_are_catchable_name_the_function_and_change_nothing() {
    let rig = Rig::new(Kind::Native);
    let other = Rig::new(Kind::Native);
    let foreign = other.token("return texture.load('a.png')");
    let texture = rig.token("return texture.load('a.png')");
    let quad = rig.token(&format!(
        "return render.create_quad{{texture='{texture}', width=4, height=4}}"
    ));
    let before = rig.service().stats();
    let long_name = "x".repeat(1025);
    let long_token = "a".repeat(65);
    let cases: Vec<(String, &str)> = vec![
        ("texture.load(5)".to_owned(), "texture.load: "),
        ("texture.load()".to_owned(), "texture.load: "),
        ("texture.load('a.png', 1)".to_owned(), "texture.load: "),
        ("texture.load('../a.png')".to_owned(), "texture.load: "),
        (format!("texture.load('{long_name}')"), "texture.load: "),
        ("texture.reload(5)".to_owned(), "texture.reload: "),
        ("texture.reload('nonsense')".to_owned(), "texture.reload: "),
        (
            format!("texture.reload('{long_token}')"),
            "texture.reload: ",
        ),
        (format!("texture.reload('{quad}')"), "texture.reload: "),
        (format!("texture.status('{quad}')"), "texture.status: "),
        (format!("texture.release('{quad}')"), "texture.release: "),
        (format!("texture.status('{foreign}')"), "texture.status: "),
        (format!("texture.reload('{foreign}')"), "texture.reload: "),
        ("render.create_quad(5)".to_owned(), "render.create_quad: "),
        ("render.create_quad{}".to_owned(), "render.create_quad: "),
        (
            format!("render.create_quad{{texture='{texture}'}}"),
            "render.create_quad: ",
        ),
        (
            format!("render.create_quad{{texture='{texture}', width=4, height=4, extra=1}}"),
            "render.create_quad: ",
        ),
        (
            format!("render.create_quad{{texture='{texture}', width=0, height=4}}"),
            "render.create_quad: ",
        ),
        (
            format!("render.create_quad{{texture='{texture}', width=4, height=4, x=1/0}}"),
            "render.create_quad: ",
        ),
        (
            format!("render.create_quad{{texture='{quad}', width=4, height=4}}"),
            "render.create_quad: ",
        ),
        (
            format!("render.create_quad{{texture='{foreign}', width=4, height=4}}"),
            "render.create_quad: ",
        ),
        (
            format!("render.set_position('{quad}', 1)"),
            "render.set_position: ",
        ),
        (
            format!("render.set_position('{quad}', 'x', 2)"),
            "render.set_position: ",
        ),
        (
            format!("render.set_position('{quad}', 0/0, 2)"),
            "render.set_position: ",
        ),
        (
            format!("render.set_position('{texture}', 1, 2)"),
            "render.set_position: ",
        ),
        (
            format!("render.set_rect('{quad}', 1, 2, 0, 4)"),
            "render.set_rect: ",
        ),
        (
            format!("render.set_rect('{quad}', 1, 2, 3)"),
            "render.set_rect: ",
        ),
        (
            format!("render.destroy_quad('{texture}')"),
            "render.destroy_quad: ",
        ),
        ("render.destroy_quad()".to_owned(), "render.destroy_quad: "),
    ];
    for (call, prefix) in &cases {
        let error = rig.error(call);
        assert_ne!(error, "ok", "{call}");
        assert!(error.starts_with(prefix), "{call}: {error}");
    }
    let after = rig.service().stats();
    assert_eq!(
        after.registry.live, before.registry.live,
        "no resource was added or kept"
    );
    assert_eq!(after.jobs.reserved, before.jobs.reserved);
    assert_eq!(after.metadata.reserved, before.metadata.reserved);
}

// ---------------------------------------------------------------------------------------------
// render: quads and publication
// ---------------------------------------------------------------------------------------------

/// Create, move, resize and destroy with their tokens; the frame published after a callback has the quads in creation order;
/// however many edits a callback makes, it publishes one frame. Negative control: a scene that copied itself per call would
/// publish once per edit (the count of snapshots would be the number of edits).
#[test]
fn quads_are_created_moved_resized_and_destroyed_and_a_callback_publishes_one_frame() {
    let rig = Rig::new(Kind::Native);
    let attached = rig.seen.count();
    let (mut owner, _dir) = rig.vm("local image = texture.load('a.png')\n\
         local first, second\n\
         function startup()\n\
           first = render.create_quad{texture = image, x = 1, y = 2, width = 10, height = 20}\n\
           second = render.create_quad{texture = image, width = 30, height = 40}\n\
         end\n\
         frame.on_update(function()\n\
           render.set_position(first, 5, 6)\n\
           render.set_rect(second, 7, 8, 9, 10)\n\
           render.set_position(first, 11, 12)\n\
           render.set_position(second, 13, 14)\n\
         end)\n\
         window.on_opened(function() render.destroy_quad(first) end)\n");
    // startup(): two creations, one frame, painter order = creation order, x and y default to 0.
    assert_eq!(
        rig.seen.count(),
        attached + 1,
        "startup published one frame"
    );
    assert_eq!(
        rig.seen.rects(),
        [[1.0, 2.0, 10.0, 20.0], [0.0, 0.0, 30.0, 40.0]]
    );
    // One callback, four edits, one frame.
    assert!(matches!(
        call(&mut owner, "frame_update"),
        CallOutcome::Returned(_)
    ));
    assert_eq!(
        rig.seen.count(),
        attached + 2,
        "four edits published one frame"
    );
    assert_eq!(
        rig.seen.rects(),
        [[11.0, 12.0, 10.0, 20.0], [13.0, 14.0, 9.0, 10.0]]
    );
    // A callback that edits nothing publishes nothing.
    assert!(matches!(
        call(&mut owner, "window_nothing"),
        CallOutcome::NotRegistered
    ));
    assert_eq!(rig.seen.count(), attached + 2);
    // Destroy: the quad leaves the next frame, the other keeps its place.
    assert!(matches!(
        call(&mut owner, "opened"),
        CallOutcome::Returned(_)
    ));
    assert_eq!(rig.seen.count(), attached + 3);
    assert_eq!(rig.seen.rects(), [[13.0, 14.0, 9.0, 10.0]]);
    let _ = owner.stop(Duration::from_secs(5));
}

/// A quad of a released texture keeps drawing (its retained reference) and blocks the texture's retirement until the quad is
/// destroyed. Negative control: if `release` ended the quad's reference too, the entry would retire at once.
#[test]
fn a_quad_of_a_released_texture_keeps_drawing_and_blocks_retirement_until_destroyed() {
    let rig = Rig::new(Kind::Native);
    let image = rig.token("return texture.load('a.png')");
    let quad = rig.token(&format!(
        "return render.create_quad{{texture='{image}', width=8, height=4}}"
    ));
    assert_eq!(rig.error(&format!("texture.release('{image}')")), "ok");
    let live = rig.service().stats().registry;
    assert_eq!(
        (live.live, live.retiring),
        (1, 0),
        "the quad's reference keeps the entry live: {live:?}"
    );
    assert!(
        rig.error(&format!("texture.status('{image}')"))
            .contains("no longer usable")
    );
    assert_eq!(
        rig.error(&format!("render.set_position('{quad}', 3, 4)")),
        "ok",
        "the quad is still usable"
    );
    assert_eq!(rig.error(&format!("render.destroy_quad('{quad}')")), "ok");
    let _ = rig.service().maintain(16);
    let after = rig.service().stats().registry;
    assert_eq!(
        after.live, 0,
        "once the quad is gone nothing holds the entry: {after:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// frame
// ---------------------------------------------------------------------------------------------

fn enabled(control: &ck_window::UpdateControl) -> bool {
    format!("{control:?}").contains("enabled: true")
}

fn epoch(control: &ck_window::UpdateControl) -> u64 {
    let text = format!("{control:?}");
    let digits = text
        .split("epoch: ")
        .nth(1)
        .and_then(|rest| rest.split([',', ' ', '}']).next())
        .expect("an epoch");
    digits.parse().expect("a number")
}

/// `frame.on_update(fn)` turns the control on and registers `frame_update`; `frame.on_update(nil)` turns it off; the observer
/// ran by the time the next statement does. Each change advances the epoch. Negative control: a plain setter leaves the
/// control off.
#[test]
fn frame_on_update_registers_frame_update_and_drives_the_control_after_the_registration() {
    let rig = Rig::new(Kind::Native);
    assert!(!enabled(&rig.control));
    // A script that registers and immediately reads the control through a host function sees it on, then off.
    let probe = {
        let control = rig.control.clone();
        Extension::new("probe").function(
            "enabled",
            Arc::new(move |_| Ok(vec![ConfigValue::Bool(enabled(&control))])),
        )
    };
    let first = epoch(&rig.control);
    let (owner, _dir) = rig.vm_with(
        "assert(probe.enabled() == false)\n\
         frame.on_update(function() end)\n\
         assert(probe.enabled() == true, 'the observer ran before the next statement')\n\
         frame.on_update(nil)\n\
         assert(probe.enabled() == false)\n\
         function startup() end\n",
        vec![probe],
    );
    drop(owner);
    assert_eq!(
        epoch(&rig.control),
        first + 2,
        "each change advanced the epoch"
    );
    let start = epoch(&rig.control);
    let (mut owner, _dir) =
        rig.vm("frame.on_update(function() return 'tick' end)\nfunction startup() end\n");
    assert!(
        enabled(&rig.control),
        "the control is on once the script registered"
    );
    assert_eq!(epoch(&rig.control), start + 1);
    match call(&mut owner, "frame_update") {
        CallOutcome::Returned(values) => {
            assert_eq!(values, [Some(ConfigValue::String("tick".into()))]);
        }
        other => panic!("{other:?}"),
    }
    let _ = owner.stop(Duration::from_secs(5));
    // nil turns it off, a second registration advances the epoch again.
    let (mut owner, _dir) =
        rig.vm("frame.on_update(function() end)\nframe.on_update(nil)\nfunction startup() end\n");
    assert!(!enabled(&rig.control));
    assert_eq!(epoch(&rig.control), start + 3);
    assert!(matches!(
        call(&mut owner, "frame_update"),
        CallOutcome::NotRegistered
    ));
    let _ = owner.stop(Duration::from_secs(5));
    // The control of a plain setter never moves (the headless composition).
    let plain = Rig::new(Kind::Headless);
    let (owner, _dir) = plain.vm("frame.on_update(function() end)\nfunction startup() end\n");
    assert!(!enabled(&plain.control));
    drop(owner);
}

// ---------------------------------------------------------------------------------------------
// glfw-vulkan
// ---------------------------------------------------------------------------------------------

/// On `glfw-vulkan` the calls that were refused until milestone 5.5 work over its backend-mode service: `texture.load` and
/// `status`, `render.create_quad`, `set_position` and `destroy_quad`, and `frame.on_update(fn)` turns the update control on
/// while `frame.on_update(nil)` turns it off again. Negative control: the same calls on a run **without** a service say so
/// (the argument errors come first), so the working calls are the service's doing, not the extensions' leniency.
#[test]
fn on_glfw_vulkan_the_new_calls_work_over_the_backend_service_and_frame_drives_the_control() {
    let rig = Rig::new(Kind::Glfw);
    assert!(!enabled(&rig.control));
    let image = rig.token("return texture.load('a.png')");
    let quad = rig.token(&format!(
        "return render.create_quad{{texture='{image}', width=8, height=4}}"
    ));
    assert_eq!(rig.error(&format!("texture.status('{image}')")), "ok");
    assert_eq!(
        rig.error(&format!("render.set_position('{quad}', 1, 2)")),
        "ok"
    );
    assert_eq!(rig.error(&format!("render.destroy_quad('{quad}')")), "ok");
    assert_eq!(rig.error(&format!("texture.release('{image}')")), "ok");
    let before = epoch(&rig.control);
    let (owner, _dir) = rig.vm("frame.on_update(function() end)\nfunction startup() end\n");
    assert!(
        enabled(&rig.control),
        "frame.on_update(fn) turned the control on"
    );
    assert_eq!(epoch(&rig.control), before + 1);
    drop(owner);
    let (_owner, _dir) =
        rig.vm("frame.on_update(function() end)\nframe.on_update(nil)\nfunction startup() end\n");
    assert!(!enabled(&rig.control), "frame.on_update(nil) turned it off");
    // A bad argument is the ordinary error, as everywhere.
    for (call, name) in [
        ("texture.load(5)", "texture.load"),
        ("render.create_quad(5)", "render.create_quad"),
        ("frame.on_update(3)", "frame.on_update"),
    ] {
        let error = rig.error(call);
        assert!(error.contains(name), "{call}: {error}");
    }
    assert_eq!(rig.error("render.quad{image='a.png'}"), "ok");
    assert_eq!(rig.error("render.clear_quad()"), "ok");
    // The control: a run without a service says so after validating the arguments.
    let none = Rig::new(Kind::NoService);
    for (call, name) in [
        ("texture.load('a.png')", "texture.load"),
        (
            "render.create_quad{texture='ck1:tex:0000000000000001:00000001:0000000000000001', width=4, height=4}",
            "render.create_quad",
        ),
    ] {
        assert_eq!(
            none.error(call),
            format!("{name}: this run has no resource service"),
            "{call}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// the legacy switch
// ---------------------------------------------------------------------------------------------

/// On the upgraded backend `render.quad` goes through the lane: the compatibility quad appears in the published frame, and a
/// refusal of the service is a catchable error that leaves the old quad. Without the flag the call is today's.
#[test]
#[expect(
    clippy::float_cmp,
    reason = "the numbers are the integers the script wrote"
)]
fn the_upgraded_flag_sends_render_quad_through_the_lane_and_a_refusal_leaves_the_old_quad() {
    // One registry slot: the compatibility quad holds it, so the second image is refused.
    let limits = ResourceLimits {
        max_textures: 1,
        ..ResourceLimits::DEFAULT
    };
    let rig = Rig::with(Kind::Native, true, limits);
    let (_owner, _dir) = rig.vm("function startup()\n\
           render.quad{image = 'a.png', x = 3, y = 4}\n\
         end\n");
    let compat = rig
        .seen
        .compat()
        .expect("the compatibility quad is in the published frame");
    assert_eq!(compat, [3.0, 4.0, 8.0, 4.0]);
    assert_eq!(
        rig.service().stats().registry.live,
        1,
        "it came through the service"
    );
    let before = rig.seen.count();
    let error = rig.error("render.quad{image = 'b.png'}");
    assert!(error.starts_with("render.quad:"), "{error}");
    assert_eq!(rig.seen.compat(), Some(compat), "the old quad is kept");
    assert_eq!(rig.seen.count(), before, "a refused call published nothing");
    // Not upgraded: today's path, no resource is admitted for the legacy quad.
    let plain = Rig::new(Kind::Native);
    assert_eq!(plain.error("render.quad{image='a.png'}"), "ok");
    assert_eq!(
        plain.service().stats().registry.live,
        0,
        "today's path never touched the service"
    );
    assert!(plain.seen.last().quad.is_some());
}

// ---------------------------------------------------------------------------------------------
// authority
// ---------------------------------------------------------------------------------------------

/// The gameplay VM has no `texture`, `frame` or new `render` functions (it has no `render` at all) and no completion hook;
/// the UI VM has all of them and its hook runs.
#[test]
fn only_the_ui_vm_gets_the_texture_frame_and_scene_api_and_the_completion_hook() {
    use ck_runtime::{Outcome, Runtime};

    use crate::config::VmConfig;
    use crate::vm::{UiWiring, spawns};

    let rig = Rig::new(Kind::Native);
    let hooks = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hooks);
    let hook = rig.end.completion_hook();
    let scripts = Dir::new();
    fs::write(
        scripts.0.join("gameplay.lua"),
        "assert(texture == nil and frame == nil and render == nil and window == nil, 'no UI API')\n\
         function startup() end\n",
    )
    .expect("a script");
    fs::write(
        scripts.0.join("ui.lua"),
        "assert(type(texture.load) == 'function' and type(frame.on_update) == 'function')\n\
         assert(type(render.create_quad) == 'function' and type(render.destroy_quad) == 'function')\n\
         function startup() end\n",
    )
    .expect("a script");
    let vms = vec![
        VmConfig {
            name: "gameplay".to_owned(),
            entry: scripts.0.join("gameplay.lua"),
        },
        VmConfig {
            name: "ui".to_owned(),
            entry: scripts.0.join("ui.lua"),
        },
    ];
    let wiring = UiWiring {
        extensions: rig.extensions.clone(),
        slot: crate::ui_link::UiSlot::default(),
        wake: Arc::new(|| {}),
        bridge: Arc::clone(&rig.bridge),
        notes: std::rc::Rc::new(crate::windowed::RunNotes::default()),
        on_complete: Some(Arc::new(move |completion| {
            counted.fetch_add(1, Ordering::SeqCst);
            hook(completion);
        })),
    };
    let spawned = spawns(&vms, &ConfigValue::Table(BTreeMap::new()), Some(&wiring));
    let report = Runtime::new(Vec::new()).with_owners(spawned).run_report();
    assert_eq!(report.outcome, Outcome::Clean, "{report:?}");
    assert_eq!(
        hooks.load(Ordering::SeqCst),
        1,
        "one hook call: the UI VM's startup (the gameplay VM has no hook)"
    );
}

// ---------------------------------------------------------------------------------------------
// headless and the run's end
// ---------------------------------------------------------------------------------------------

/// Headless: the scene calls apply to the editable scene and publish into the bridge; the first accepted one logs the
/// adapted record, the rest are counted; texture calls are admitted and `status` reaches `cpu_ready`.
#[test]
fn headless_scene_calls_log_the_adapted_notice_once_and_count_the_rest() {
    let rig = Rig::new(Kind::Headless);
    let texture = rig.token("return texture.load('a.png')");
    assert!(
        rig.records.lock().unwrap().is_empty(),
        "texture calls are not scene calls"
    );
    let quad = rig.token(&format!(
        "return render.create_quad{{texture='{texture}', width=4, height=4}}"
    ));
    assert_eq!(
        *rig.records.lock().unwrap(),
        [(
            ck_log::Level::Info,
            "render",
            "scene calls ignored: headless run".to_owned()
        )]
    );
    assert_eq!(
        rig.error(&format!("render.set_position('{quad}', 1, 2)")),
        "ok"
    );
    assert_eq!(
        rig.error(&format!("render.set_rect('{quad}', 1, 2, 3, 4)")),
        "ok"
    );
    assert_eq!(rig.error(&format!("render.destroy_quad('{quad}')")), "ok");
    assert_eq!(
        rig.records.lock().unwrap().len(),
        1,
        "later calls are counted, not logged"
    );
    assert_eq!(
        rig.notes.final_count().as_deref(),
        Some("3 further render request(s) ignored: headless run")
    );
    // The same script publishes into the bridge (stored, nothing draws it).
    let (_owner, _dir) = rig.vm(
        "local t = texture.load('a.png')\nfunction startup() render.create_quad{texture = t, width = 2, height = 2} end\n",
    );
    assert_eq!(rig.seen.rects(), [[0.0, 0.0, 2.0, 2.0]]);
}

/// After the run's end the texture calls fail with the service's `Ended` error and do no I/O; the new scene calls raise it
/// too and **retain nothing**; only `render.quad` keeps M4's semantics. Negative control: a scene call that applied its edit
/// after the end leaves a referenced entry at `finish_teardown`.
#[test]
fn after_the_end_texture_and_scene_calls_raise_ended_and_retain_nothing() {
    for kind in [Kind::Native, Kind::Headless] {
        let rig = Rig::new(kind);
        let texture = rig.token("return texture.load('a.png')");
        let quad = rig.token(&format!(
            "return render.create_quad{{texture='{texture}', width=4, height=4}}"
        ));
        // Admission closes (CK-124's step 2); the writer still exists.
        let service = rig.service().clone();
        service.close();
        // The assets root is gone: a call that did I/O would fail differently.
        fs::remove_dir_all(&rig.root).expect("the assets root");
        let published = rig.seen.count();
        let stats = service.stats();
        for call in [
            "texture.load('a.png')".to_owned(),
            format!("texture.reload('{texture}')"),
            format!("texture.release('{texture}')"),
            format!("texture.status('{texture}')"),
            format!("render.create_quad{{texture='{texture}', width=4, height=4}}"),
            format!("render.set_position('{quad}', 1, 2)"),
            format!("render.set_rect('{quad}', 1, 2, 3, 4)"),
            format!("render.destroy_quad('{quad}')"),
        ] {
            let error = rig.error(&call);
            assert!(
                error.contains("the resource service has ended"),
                "{call}: {error}"
            );
        }
        let after = service.stats();
        assert_eq!(
            after.registry, stats.registry,
            "nothing was retained or released"
        );
        assert_eq!(rig.seen.count(), published, "nothing was published");
        // Only the legacy calls keep M4's semantics: accepted after the bridge ended (they decode and discard).
        rig.bridge.detach();
        let (_owner, _dir) = rig.vm("function startup() end\n");
        assert_eq!(rig.seen.count(), published);
        // After the writer's end the slot is empty and every scene call raises Ended too.
        rig.end.end();
        assert!(
            rig.error(&format!("render.destroy_quad('{quad}')"))
                .contains("ended")
        );
        assert!(rig.error("render.clear_quad()").contains("run has ended"));
    }

    // Negative control: an edit applied after the end holds a reference at `finish_teardown`.
    let rig = Rig::new(Kind::Native);
    let texture = rig.token("return texture.load('a.png')");
    let key = TextureKey::from_token(&texture).expect("a token");
    let reference = rig.service().retain(key).expect("retained");
    rig.service().close();
    rig.end.add_test_quad(reference);
    let _ = rig.service().finish_teardown();
    assert_ne!(
        rig.service().stats().loaders.referenced_at_teardown,
        0,
        "an edit after the end is exactly what the Ended check prevents"
    );
}

/// The writer's lock is taken only on the UI thread during the run and once by `end()` after it; after `end()` the slot is
/// empty and every scene call raises `Ended`.
#[test]
fn the_writer_lock_is_taken_only_by_the_ui_thread_during_the_run_and_once_by_end() {
    let rig = Rig::new(Kind::Native);
    let (mut owner, _dir) = rig.vm("local t = texture.load('a.png')\n\
         local q\n\
         function startup() q = render.create_quad{texture = t, width = 3, height = 3} end\n\
         frame.on_update(function() render.set_position(q, 1, 1) end)\n");
    assert!(matches!(
        call(&mut owner, "frame_update"),
        CallOutcome::Returned(_)
    ));
    let _ = owner.stop(Duration::from_secs(5));
    assert!(owner.wait_finished(Duration::from_secs(5)));
    let during = rig.end.lock_threads();
    assert!(!during.is_empty());
    let ui = during[0];
    assert!(
        during.iter().all(|thread| *thread == ui),
        "only the UI VM's thread: {during:?}"
    );
    assert_ne!(ui, std::thread::current().id());
    rig.end.end();
    let all = rig.end.lock_threads();
    assert_eq!(all.len(), during.len() + 1, "end() took it once");
    assert_eq!(all.last().copied(), Some(std::thread::current().id()));
    assert!(
        rig.error(
            "render.set_position('ck1:quad:0000000000000001:00000001:0000000000000001', 1, 2)"
        )
        .contains("ended")
            || rig
                .error("render.create_quad{texture='x', width=1, height=1}")
                .contains("render.create_quad")
    );
}

/// A frame callback that moves a quad and then raises still publishes the move (the hook runs on failure too); the run's
/// exit status 1 on such a failure is CK-122/CK-123's path (`a_raising_frame_update_ends_the_run_failed_and_is_not_retried`).
#[test]
fn a_callback_that_moves_a_quad_then_raises_still_publishes_the_move() {
    let rig = Rig::new(Kind::Native);
    let (mut owner, _dir) = rig.vm(
        "local t = texture.load('a.png')\n\
         local q\n\
         function startup() q = render.create_quad{texture = t, x = 1, y = 1, width = 5, height = 5} end\n\
         frame.on_update(function()\n\
           render.set_position(q, 50, 60)\n\
           error('update boom')\n\
         end)\n",
    );
    assert_eq!(rig.seen.rects(), [[1.0, 1.0, 5.0, 5.0]]);
    match call(&mut owner, "frame_update") {
        CallOutcome::Failed { message, .. } => {
            assert!(message.contains("update boom"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        rig.seen.rects(),
        [[50.0, 60.0, 5.0, 5.0]],
        "the earlier edit was published although the callback failed"
    );
    let _ = owner.stop(Duration::from_secs(5));
}

// ---------------------------------------------------------------------------------------------
// review 1: the order of the checks, and the writer lock around the legacy lane's I/O
// ---------------------------------------------------------------------------------------------

/// A well-formed texture token and quad token that name nothing in the rig's registry (foreign): they pass the syntax
/// checks, so only an earlier or later *value* check decides the error.
const TEXTURE_TOKEN: &str = "ck1:tex:0000000000000001:00000001:0000000000000001";
const QUAD_TOKEN: &str = "ck1:quad:0000000000000001:00000001:0000000000000001";

/// Calls whose cheap values are bad, with the text each error must have, whatever the backend or the end of the run.
fn cheap_value_errors() -> Vec<(String, &'static str, &'static str)> {
    let long_name = "x".repeat(1025);
    vec![
        (
            "texture.load('../a.png')".to_owned(),
            "texture.load: ",
            "../a.png",
        ),
        (
            "texture.load('/etc/passwd')".to_owned(),
            "texture.load: ",
            "/etc/passwd",
        ),
        (
            format!("texture.load('{long_name}')"),
            "texture.load: ",
            "1024",
        ),
        (
            format!("render.create_quad{{texture='{TEXTURE_TOKEN}', width=0, height=4}}"),
            "render.create_quad: ",
            "width",
        ),
        (
            format!("render.create_quad{{texture='{TEXTURE_TOKEN}', width=4, height=-1}}"),
            "render.create_quad: ",
            "height",
        ),
        (
            format!("render.create_quad{{texture='{TEXTURE_TOKEN}', x=1/0, width=4, height=4}}"),
            "render.create_quad: ",
            "finite",
        ),
        (
            format!("render.set_position('{QUAD_TOKEN}', 0/0, 1)"),
            "render.set_position: ",
            "finite",
        ),
        (
            format!("render.set_rect('{QUAD_TOKEN}', 1, 2, 3, 0)"),
            "render.set_rect: ",
            "height",
        ),
        (
            format!("render.set_rect('{QUAD_TOKEN}', 1, 2, 3e12, 4)"),
            "render.set_rect: ",
            "width",
        ),
    ]
}

/// The cheap value checks (the name rules and the byte cap, the numbers of a rectangle) come before the no-service refusal
/// and before the end's: in a run without a service, after the run's end and in a live one the same error, never "no
/// resource service" or `Ended`; a call with good values still gets the refusal (the control).
#[test]
fn cheap_value_errors_come_before_the_backend_and_the_end_refusal() {
    let glfw = Rig::new(Kind::NoService);
    let ended = Rig::new(Kind::Native);
    ended.service().close();
    let native = Rig::new(Kind::Native);
    for (call, prefix, needle) in cheap_value_errors() {
        for (label, rig) in [
            ("no service", &glfw),
            ("ended", &ended),
            ("native", &native),
        ] {
            let error = rig.error(&call);
            assert!(error.starts_with(prefix), "{label}: {call}: {error}");
            assert!(error.contains(needle), "{label}: {call}: {error}");
            assert!(
                !error.contains("no resource service") && !error.contains("has ended"),
                "{label}: {call}: {error}"
            );
        }
    }
    // The control: good values get the refusal of the backend or of the end.
    assert!(
        glfw.error("texture.load('a.png')")
            .contains("this run has no resource service")
    );
    assert!(
        glfw.error(&format!(
            "render.create_quad{{texture='{TEXTURE_TOKEN}', width=4, height=4}}"
        ))
        .contains("this run has no resource service")
    );
    assert!(
        ended
            .error(&format!("render.set_position('{QUAD_TOKEN}', 1, 2)"))
            .contains("the resource service has ended")
    );
    assert!(
        ended
            .error("texture.load('a.png')")
            .contains("the resource service has ended")
    );
}

/// Nothing is retained before the geometry is valid: a **released** texture token with a bad rectangle reports the
/// rectangle (a retain first would have reported the stale token), and with a good rectangle the stale token; the
/// registry and the job slots never change.
#[test]
fn create_quad_checks_the_rectangle_before_it_takes_a_reference() {
    let rig = Rig::new(Kind::Native);
    let texture = rig.token("return texture.load('a.png')");
    assert_eq!(rig.error(&format!("texture.release('{texture}')")), "ok");
    let before = rig.service().stats();
    let bad = rig.error(&format!(
        "render.create_quad{{texture='{texture}', width=0, height=4}}"
    ));
    assert!(
        bad.contains("width") && !bad.contains("no longer usable"),
        "{bad}"
    );
    let good = rig.error(&format!(
        "render.create_quad{{texture='{texture}', width=4, height=4}}"
    ));
    assert!(good.contains("no longer usable"), "{good}");
    let after = rig.service().stats();
    assert_eq!(
        after.registry, before.registry,
        "no reference was left behind"
    );
    assert_eq!(after.jobs.reserved, before.jobs.reserved);
}

/// The upgraded `render.quad` reads and decodes the image **without the writer's lock**: the seam right before and right
/// after the lane's admission (the I/O) finds the lock free, by `try_lock` and not by time; the quad is installed all the
/// same. Negative control: a lock held across the admission would fail the seam's assertion.
#[test]
fn the_upgraded_render_quad_holds_no_writer_lock_while_the_lane_reads_and_decodes() {
    let rig = Rig::with(Kind::Native, true, ResourceLimits::DEFAULT);
    let points = Arc::new(Mutex::new(Vec::new()));
    {
        let (end, points) = (rig.end.clone(), Arc::clone(&points));
        rig.end.set_lane_seam(Arc::new(move |point| {
            assert!(
                end.writer_lock_is_free(),
                "the writer's lock was held {point} the lane's admission"
            );
            points
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(point);
        }));
    }
    assert_eq!(
        rig.error("render.quad{image = 'a.png', x = 3, y = 4}"),
        "ok"
    );
    assert_eq!(
        *points.lock().unwrap(),
        ["before_admit", "after_admit"],
        "the seam ran on both sides of the admission"
    );
    // The hook (the completion of a callback) publishes the compatibility quad as before.
    let (_owner, _dir) = rig.vm("function startup() render.quad{image = 'b.png'} end\n");
    assert!(rig.seen.compat().is_some());
    assert_eq!(
        points.lock().unwrap().len(),
        4,
        "the second call passed through the seam too"
    );
    // A failure is the same refusal, with the lock free again.
    let error = rig.error("render.quad{image = 'missing.png'}");
    assert!(error.starts_with("render.quad:"), "{error}");
    assert!(rig.end.writer_lock_is_free());
    // After the writer's end the call says so, without having read the image.
    rig.end.end();
    assert!(
        rig.error("render.quad{image = 'a.png'}")
            .contains("run has ended")
    );
}

/// CK-129 review 2 (F2): the first load is stamped **before** the request is published to the loaders. The service wakes the loaders
/// from inside `service.load`, on the caller's thread, after the request is queued; here that wake holds the caller until the work
/// has been decoded and "adopted" by the stand-ins for the other threads (the test thread, with the hub's own clock). Whatever the
/// caller does after the wake, the stamp must precede the ready time. Negative control: stamping after `service.load` returns (the old
/// order) gives a stamp after the ready time.
#[test]
fn the_first_load_is_stamped_before_other_threads_can_finish_it() {
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc;
    use std::thread;

    use crate::metrics::MetricsHub;

    let rig = Rig::new(Kind::Headless);
    let tick = Arc::new(AtomicU64::new(1_000));
    let clock: ck_window::UptimeNs = {
        let tick = Arc::clone(&tick);
        Arc::new(move || tick.fetch_add(1, Ordering::SeqCst))
    };
    let hub = Arc::new(MetricsHub::new(clock, false));
    let (enqueued_tx, enqueued_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    let resume_rx = Mutex::new(resume_rx);
    let wake: ck_resources::ServiceWake = Arc::new(move || {
        // The request is queued; the caller stays here until the others have finished the work.
        let _ = enqueued_tx.send(());
        let _ = resume_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recv();
    });
    let service = ResourceService::with_assets(
        ResourceLimits::DEFAULT,
        Mode::Headless,
        ServiceWakes {
            jobs: Some(wake),
            uploads: None,
            retirements: None,
        },
        Some(Assets::new(rig.root.clone())),
    )
    .expect("a service");
    let extensions = vec![
        rig.extensions[0].clone(),
        texture_api::extension_noting(Some(service.clone()), Some(hub.load_notes())),
    ];
    let caller = thread::spawn(move || run_in_ui(&extensions, "return texture.load('a.png')"));
    enqueued_rx.recv().expect("the load was queued");
    // The loaders decode it and the render thread adopts it before the caller continues.
    while let Some(job) = service.take_job() {
        service.complete(job.run());
    }
    let ready = hub.now();
    resume_tx.send(()).expect("the caller is waiting");
    caller.join().expect("the call returned");
    let first_load = hub.report().first_load_ns;
    assert!(first_load != 0, "the accepted load defined the start");
    assert!(
        first_load < ready,
        "the stamp {first_load} precedes the ready time {ready}, though the work finished before the caller continued"
    );
}

/// CK-129 review 2 (F2): a load the service refuses never defines the span. One registry slot is taken, so the script's load is
/// refused after it was stamped: the start stays unset. Negative control: telling the hub before the outcome is known sets it.
#[test]
fn a_refused_load_never_defines_the_upload_span() {
    use crate::metrics::MetricsHub;

    let limits = ResourceLimits {
        max_textures: 1,
        ..ResourceLimits::DEFAULT
    };
    let rig = Rig::with(Kind::Headless, false, limits);
    rig.service().load("b.png").expect("takes the one slot");
    let hub = Arc::new(MetricsHub::new(Arc::new(|| 5_000), false));
    let extensions = vec![
        rig.extensions[0].clone(),
        texture_api::extension_noting(rig.service.clone(), Some(hub.load_notes())),
    ];
    let error = error_in_ui(&extensions, "texture.load('a.png')");
    assert!(error.contains("texture.load"), "{error}");
    assert_eq!(
        hub.report().first_load_ns,
        0,
        "the refused load defined nothing"
    );
}
