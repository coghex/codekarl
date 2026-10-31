//! Test support for the executable's unit tests: a scratch directory, a gate, and a harness that runs a
//! real UI VM with the `window` extension over a real request slot (test builds only).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ck_lua::{CallOutcome, ConfigValue, Extension, HostFunction, VmOwner, VmSettings};
use ck_window::{LoopSettings, Notifier, Requests};

use crate::render_api;
use crate::ui_link::{CodekarlUi, UiSlot, UiVm};
use crate::window_api::{self, HeadlessNotes};
use crate::windowed::{RunEnv, RunNotes};

/// A scratch directory for scripts, removed on drop.
pub(crate) struct Dir(pub(crate) PathBuf);

impl Dir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "codekarl-windowed-{}-{}",
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

/// A gate a test opens: threads wait on it with a bound, so a bug fails a test instead of hanging it.
pub(crate) struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            changed: Condvar::new(),
        })
    }

    pub(crate) fn open(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    pub(crate) fn wait(&self) {
        let give_up = Instant::now() + Duration::from_secs(20);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open && Instant::now() < give_up {
            open = self
                .changed
                .wait_timeout(open, Duration::from_millis(20))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

pub(crate) const WINDOW: &str =
    "window.open{title='t', width=100, height=80, clear={0.1, 0.2, 0.3}}";

/// The pieces of a run, with a real UI VM running `script`.
pub(crate) struct Harness {
    _dir: Dir,
    pub(crate) requests: Requests,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) quit: Arc<AtomicBool>,
    pub(crate) ui: CodekarlUi,
    pub(crate) vm: UiVm,
    pub(crate) notes: Rc<RunNotes>,
    pub(crate) bridge: Arc<ck_render_2d::SceneBridge>,
}

/// What a test adds to the UI VM.
#[derive(Default)]
pub(crate) struct Extras {
    pub(crate) functions: Vec<(&'static str, HostFunction)>,
    /// Called on the VM's owner thread after the notifier's wake, on every report wake.
    pub(crate) after_wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The assets directory the `render` extension is configured with (none: as with no configuration file).
    pub(crate) assets: Option<PathBuf>,
    /// Headless notes: the run is headless, so `render` writes nothing and counts into them.
    pub(crate) headless: Option<Arc<HeadlessNotes>>,
    /// More extensions for the UI VM (for example [`frame_extension`]).
    pub(crate) extensions: Vec<Extension>,
    /// A resource service: the UI VM then also gets the `texture` extension and the new scene calls work, as in a real
    /// run (`None`: as on `glfw-vulkan`).
    pub(crate) service: Option<ck_resources::ResourceService>,
}

/// A test-only extension that registers a `frame_update` callback, as `window.on_close_request` registers
/// `close_request` (the Lua `frame.on_update` of milestone 5-8 is not built yet): `frame.on_update(fn)` sets it and
/// `frame.on_update(nil)` clears it.
pub(crate) fn frame_extension() -> Extension {
    Extension::new("frame").callback("on_update", window_api::FRAME_UPDATE)
}

impl Harness {
    pub(crate) fn new(script: &str, extras: Extras) -> Self {
        let dir = Dir::new();
        let entry = dir.0.join("main.lua");
        fs::write(&entry, script).expect("a script");
        let notifier = Notifier::new();
        let requests = Requests::new();
        let mut extension = window_api::extension(requests.writer(Arc::clone(&notifier)), None);
        for (name, function) in extras.functions {
            extension = extension.function(name, function);
        }
        let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        let writer = bridge.take_writer().expect("the one writer");
        let (render, render_end) = render_api::extension_with_end(
            extras.assets,
            writer,
            extras.headless,
            extras.service.clone(),
            false,
        );
        let mut settings = VmSettings::new("ui", entry, ConfigValue::Table(BTreeMap::new()));
        // The completion hook the real UI VM has: the scene is published after startup and after every posted callback.
        settings.on_complete = Some(render_end.completion_hook());
        settings.extensions = vec![extension, render];
        if extras.service.is_some() {
            settings
                .extensions
                .push(crate::texture_api::extension(extras.service));
        }
        settings.extensions.extend(extras.extensions);
        settings.call_limit = 4;
        let wake_notifier = Arc::clone(&notifier);
        let after = extras.after_wake;
        settings.on_report = Some(Arc::new(move || {
            wake_notifier.wake();
            if let Some(after) = &after {
                after();
            }
        }));
        let mut owner = VmOwner::spawn(settings).expect("the UI VM spawns");
        owner.wait_ready(Duration::from_secs(5)).expect("ready");
        owner.start(Duration::from_secs(5)).expect("started");
        let vm: UiVm = Rc::new(RefCell::new(owner));
        let slot = UiSlot::default();
        *slot.borrow_mut() = Some(Rc::clone(&vm));
        Self {
            _dir: dir,
            requests,
            notifier,
            quit: Arc::new(AtomicBool::new(false)),
            ui: CodekarlUi::new(slot),
            vm,
            notes: Rc::new(RunNotes::default()),
            bridge,
        }
    }

    pub(crate) fn env(&self) -> RunEnv {
        RunEnv {
            ui: self.ui.clone(),
            reader: self.requests.reader(),
            notifier: Arc::clone(&self.notifier),
            quit: Arc::clone(&self.quit),
            settings: LoopSettings::new(
                64,
                16,
                Duration::from_millis(200),
                Duration::from_millis(100),
            )
            .expect("settings"),
            metrics: None,
            notes: Rc::clone(&self.notes),
            bridge: Arc::clone(&self.bridge),
            wake_relay: ck_render_2d::WakeRelay::default(),
            resources: None,
            resource_config: crate::config::ResourceConfig::default(),
            max_quads: 1024,
            bindless_texture_slots: crate::config::DEFAULT_BINDLESS_TEXTURE_SLOTS,
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let mut vm = self.vm.borrow_mut();
        vm.cancel();
        let _ = vm.wait_finished(Duration::from_secs(5));
    }
}

/// A PNG of `width` x `height` RGBA8 pixels (`rgba` holds them, rows top first), written with stored (not
/// compressed) deflate blocks: enough for the application's tests to give `render.quad` a real file without a
/// PNG encoder.
pub(crate) fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
        out.extend_from_slice(
            &u32::try_from(data.len())
                .expect("a small chunk")
                .to_be_bytes(),
        );
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0); // the filter type: none
        raw.extend_from_slice(row);
    }
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "pixels for the size"
    );
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        let length = u16::try_from(block.len()).expect("a stored block");
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut out, *b"IHDR", &header);
    chunk(&mut out, *b"IDAT", &zlib);
    chunk(&mut out, *b"IEND", &[]);
    out
}

/// Runs `body` (Lua statements returning values) inside the `opened` callback of a VM named `ui` with
/// `extensions` (which must include `window`, for `window.on_opened`) and returns what it returned.
pub(crate) fn run_in_ui(extensions: &[Extension], body: &str) -> Vec<Option<ConfigValue>> {
    let dir = Dir::new();
    let entry = dir.0.join("main.lua");
    fs::write(
        &entry,
        format!("window.on_opened(function()\n{body}\nend)\n"),
    )
    .expect("a script");
    let mut settings = VmSettings::new("ui", entry, ConfigValue::Table(BTreeMap::new()));
    settings.extensions = extensions.to_vec();
    settings.call_limit = 4;
    let mut vm = VmOwner::spawn(settings).expect("the VM spawns");
    vm.wait_ready(Duration::from_secs(5)).expect("ready");
    vm.start(Duration::from_secs(5)).expect("started");
    vm.post(window_api::OPENED, Vec::new()).expect("posted");
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

/// The error text of `pcall(function() <call> end)` run by [`run_in_ui`], or `ok`. The traceback (which names
/// the scratch directory) is cut off.
pub(crate) fn error_in_ui(extensions: &[Extension], call: &str) -> String {
    let values = run_in_ui(
        extensions,
        &format!(
            "local ok, err = pcall(function() {call} end)\nif ok then return 'ok' end\nreturn tostring(err)"
        ),
    );
    let Some(Some(ConfigValue::String(full))) = values.first() else {
        panic!("not a string: {values:?}");
    };
    full.split("\nstack traceback")
        .next()
        .unwrap_or_default()
        .to_owned()
}

// ---------------------------------------------------------------------------------------------
// The headless driver (CK-126)
// ---------------------------------------------------------------------------------------------

/// One reading of a texture as the probe reported it: the name it was loaded with, the state `texture.status` gave and the
/// error text it gave (empty: none) and the revision requested so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) error: String,
    pub(crate) requested: i64,
}

/// Lua prepended to a script the driver runs: it remembers the token of every `texture.load` by name, and after each
/// `frame_update` callback reports every texture's status to the test through `window.probe`. A script needs no change.
pub(crate) const PROBE_PRELUDE: &str = r"
local tokens, order = {}, {}
local real_load = texture.load
texture.load = function(name)
  local token = real_load(name)
  if tokens[name] == nil then order[#order + 1] = name end
  tokens[name] = token
  return token
end
local function probe_all()
  for _, name in ipairs(order) do
    local ok, status = pcall(texture.status, tokens[name])
    if ok then window.probe(name, status.state, status.error or '', status.requested) end
  end
end
local real_on_update = frame.on_update
frame.on_update = function(callback)
  if callback == nil then return real_on_update(nil) end
  return real_on_update(function(f)
    callback(f)
    probe_all()
  end)
end
";

/// The whole-case deadline of a driven run: a bug fails the case instead of hanging it.
const CASE_DEADLINE: Duration = Duration::from_secs(10);

/// The actions of the default bindings, every one reported with nothing held (the real update reports every action name).
const DEFAULT_ACTIONS: [&str; 6] = [
    "close",
    "move_down",
    "move_left",
    "move_right",
    "move_up",
    "reload_texture",
];

/// What the driver's closure may do while the run is between its start-up and its stop phase.
pub(crate) struct Drive {
    ui: CodekarlUi,
    /// The run's resource service (the observable state the driver waits on).
    pub(crate) service: ck_resources::ResourceService,
    readings: Arc<Mutex<Vec<Reading>>>,
    started: Instant,
    sequence: u64,
}

impl Drive {
    /// Posts the UI's `frame_update` callback through the same posting path the window loop uses, with a fixed `dt` and
    /// every default action idle, and waits for its answer (which must be a normal return).
    pub(crate) fn post_update(&mut self) {
        self.post_update_with(&[]);
    }

    /// [`post_update`](Self::post_update) with the named actions pressed (and held) in the snapshot, as a bound key would give.
    pub(crate) fn post_update_with(&mut self, pressed: &[&str]) {
        use ck_window::{
            ActionSnapshot, Answer, InputSnapshot, PixelSize, UiEvent, UiLink, UpdateEvent,
            WindowMetrics,
        };
        self.sequence += 1;
        let step = Duration::from_millis(16);
        let update = UpdateEvent {
            sequence: self.sequence,
            elapsed: step,
            dt: step,
            metrics: WindowMetrics {
                points: (800, 600),
                pixels: PixelSize {
                    width: 1600,
                    height: 1200,
                },
            },
            input: InputSnapshot {
                sequence: u128::from(self.sequence),
                actions: DEFAULT_ACTIONS
                    .iter()
                    .map(|name| ActionSnapshot {
                        name: (*name).to_owned(),
                        held: pressed.contains(name),
                        pressed: pressed.contains(name),
                        released: false,
                    })
                    .collect(),
                transitions: Vec::new(),
                overflow: false,
                reset: false,
                overflow_count: 0,
            },
        };
        self.ui
            .post(UiEvent::Update(update))
            .expect("the update is posted");
        loop {
            if let Some(answer) = self.ui.take_answer() {
                assert_eq!(
                    answer.answer,
                    Answer::Returned,
                    "the frame_update callback returned"
                );
                return;
            }
            self.check_deadline("the frame_update answer");
            std::thread::yield_now();
        }
    }

    /// Waits until `condition` holds on the service's own state, failing the case at its deadline.
    pub(crate) fn wait_until(&mut self, what: &str, mut condition: impl FnMut(&Self) -> bool) {
        while !condition(self) {
            self.check_deadline(what);
            std::thread::yield_now();
        }
    }

    fn check_deadline(&self, what: &str) {
        assert!(
            self.started.elapsed() < CASE_DEADLINE,
            "gave up waiting for {what} after {CASE_DEADLINE:?}"
        );
    }

    /// The latest reading of every texture, by name.
    pub(crate) fn latest(&self) -> BTreeMap<String, Reading> {
        latest_readings(&self.readings.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn latest_readings(readings: &[Reading]) -> BTreeMap<String, Reading> {
    readings
        .iter()
        .map(|reading| (reading.name.clone(), reading.clone()))
        .collect()
}

/// What a driven run left behind.
pub(crate) struct DrivenRun {
    pub(crate) status: crate::Status,
    /// The run's resource service, after the run.
    pub(crate) service: ck_resources::ResourceService,
    /// Every reading the probe reported, in order.
    pub(crate) readings: Vec<Reading>,
}

impl DrivenRun {
    /// The latest reading of every texture, by name.
    pub(crate) fn latest(&self) -> BTreeMap<String, Reading> {
        latest_readings(&self.readings)
    }
}

/// The headless composition of milestone 5-8a (service, loader pool, bridge, both VMs) run with a given configuration and
/// UI script, with a closure that runs between start-up and the stop phase and may post the UI's `frame_update` itself: the
/// production headless run posts none (no window loop exists), so this is how the incremental workloads are exercised.
pub(crate) struct HeadlessDriver {
    /// The assets directory the run reads textures from.
    pub(crate) assets: PathBuf,
    /// The UI script (the probe prelude is added to it).
    pub(crate) script: String,
    /// Extra configuration text (for example a `[runtime]` table).
    pub(crate) config: String,
}

impl HeadlessDriver {
    /// Runs the composition: `drive` runs on the run thread after the VMs started; the run then ends normally (admission
    /// closes, the VMs' `shutdown()` runs, the pool stops, the service reconciles).
    pub(crate) fn run(self, drive: impl FnOnce(&mut Drive) + 'static) -> DrivenRun {
        let dir = Dir::new();
        let ui = dir.0.join("ui.lua");
        fs::write(&ui, format!("{PROBE_PRELUDE}\n{}", self.script)).expect("the ui script");
        let config = crate::config::load_text_for_test(&format!(
            "{}\n[render]\nassets = {:?}\n\n[lua.ui]\nentry = {ui:?}\n",
            self.config, self.assets
        ));
        let readings: Arc<Mutex<Vec<Reading>>> = Arc::new(Mutex::new(Vec::new()));
        let service_out: Arc<Mutex<Option<ck_resources::ResourceService>>> =
            Arc::new(Mutex::new(None));
        let probe_into = Arc::clone(&readings);
        let probe: HostFunction = Arc::new(move |arguments| {
            let text = |index: usize| match arguments.get(index) {
                Some(Some(ConfigValue::String(text))) => text.clone(),
                _ => String::new(),
            };
            probe_into
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Reading {
                    name: text(0),
                    state: text(1),
                    error: text(2),
                    requested: match arguments.get(3) {
                        Some(Some(ConfigValue::Integer(revision))) => *revision,
                        _ => -1,
                    },
                });
            Ok(Vec::new())
        });
        let service_slot = Arc::clone(&service_out);
        let readings_for_drive = Arc::clone(&readings);
        let status = crate::supervise_with_owner_factory(
            &config,
            crate::mode::Mode::Headless,
            crate::Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            |_| panic!("a headless run has no window plan"),
            move |setup| {
                let extension = setup.extension.function("probe", probe);
                let env = setup.env;
                let service = env.resources.clone().expect("a headless run has a service");
                *service_slot.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(service.clone());
                crate::Plan {
                    extension,
                    run: Some(Box::new(move |_| {
                        let mut driving = Drive {
                            ui: env.ui.clone(),
                            service,
                            readings: readings_for_drive,
                            started: Instant::now(),
                            sequence: 0,
                        };
                        drive(&mut driving);
                        env.reader.end();
                        ck_runtime::RunEnd::Finished
                    })),
                }
            },
            |wiring, _, _| crate::vm::spawns(&config.vms, &config.whole, Some(wiring)),
        );
        let service = service_out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("the driver saw the service");
        let readings = readings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        DrivenRun {
            status,
            service,
            readings,
        }
    }
}
