//! `Renderer<MetalBackend>` on a real `ck-window-cocoa` window, driven by `ck_window::run` with a fake UI.
//!
//! `AppKit` needs the process's main thread, so this executable is a **supervisor and a worker** (`harness = false`):
//! the supervisor starts it again as the worker, with Metal's API and shader validation on, gives it a whole-process
//! deadline, kills and reaps it on every path, and fails on any validation report in its output. The worker:
//!
//! 1. sets a quad on the scene bridge **before** the window opens (generation 1; the attach is generation 2);
//! 2. runs `ck_window::run` over a `CocoaBackend`, the renderer and a scripted UI link;
//! 3. when the first frame's generation is observed through `drawn_generation` (it must be 2: the quad written
//!    before the window opened is in the first frame), clears the quad (generation 3);
//! 4. when that scene's generation is observed (a frame with no quad, drawn with its own generation), closes the
//!    window; the loop's teardown must prove the presenter's end (`LoopEnd::Closed`) and the bridge is ended.
//!
//! **Typing.** The owner may be typing while this runs, Escape included. Each worker installs, before its first
//! window, a test-only filter (`tests/native/testing.m`, linked only into this test) that drops every keyboard
//! event before it reaches a window or responder, and every scenario runs twice: on a quiet keyboard and with a
//! thread typing without stopping (Escape, a repeat, a key-up, a modifier change). A worker fails if a close
//! request reached its UI, and in the typing run it waits until the filter has dropped keys. The supervisor
//! also proves the filter is in this test's archive and in no library artifact.
//!
//! A second worker run (`identity`) starts the real presenter with the backend's own sink conversion
//! (`sink_over_slot`) and the presenter's observer, and checks the **source identity** the sink promises: the same
//! image republished with a moved quad is one upload, a new image is one more.
//!
//! The scripted UI still declines every close request, and nothing here depends on a quiet keyboard. A ticker
//! thread wakes the loop every few milliseconds (a test-only stimulus; frames after the first produce no report
//! that would wake it).

use std::env;
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_metal::{
    BindlessLimits, CapturedFrame, CleanupResult as GpuCleanup, FrameCounters, LedgerLimits,
    MetalContext, MetalPresenter, MetalQuadSlot, Observer, PresenterReport as GpuReport,
    SceneStats, TestHooks, TextureLimits, TextureTestHooks,
};
use ck_render_2d::{
    BackendTestHooks, Filter, Gpu2d, Image, LegacyLane, MetalBackend, Phase, Quad, QuadKey,
    QuadSink, Renderer, ResourceBackendLimits, SceneBridge, SceneWriter, WakeRelay, WriteCounts,
    sink_over_slot,
};
use ck_resources::{
    Assets, CpuPixels, LoaderPool, Mode, ResourceLimits, ResourceService, ServiceWakes,
    StatusState, TextureKey,
};
use ck_window::{
    Answer, CleanupResult, LoopEnd, LoopSettings, Notifier, PixelSize, PostError, PresenterReport,
    RequestWriter, Requests, UiAnswer, UiEvent, UiLink, WindowDesc, run,
};
use ck_window_cocoa::{CocoaBackend, CocoaHandles};
#[path = "../../window/tests/support/activation_watch.rs"]
mod activation_watch;

const WORKER_VARIABLE: &str = "CK_RENDER2D_METAL_WINDOW_WORKER";
/// `1` makes a worker run with the owner typing without stopping.
const TYPING_VARIABLE: &str = "CK_RENDER2D_METAL_WINDOW_TYPING";
/// The longest the worker may run.
const DEADLINE: Duration = Duration::from_secs(60);
/// What Metal's validation layers print when something is wrong.
const VALIDATION_MARKERS: [&str; 4] = [
    "failed assertion",
    "MTLDebug",
    "Validation failed",
    "validation error",
];
/// The generation of the first frame: the write before the window opened is generation 1, the attach 2.
const FIRST: u64 = 2;

/// The worker scenarios, named by the worker variable's value.
const SCENARIOS: [&str; 11] = [
    "basic",
    "identity",
    "res-lifecycle",
    "res-delayed-checker",
    "res-checker-failure",
    "res-early-close",
    "res-early-close-retained",
    "res-relay",
    "res-movement-reupload",
    "res-oracles",
    "res-compat",
];

unsafe extern "C" {
    fn ck_r2_t_install_input_filter();
    fn ck_r2_t_dropped_keys() -> u64;
    fn ck_r2_t_start_typist();
}

fn typing() -> bool {
    env::var_os(TYPING_VARIABLE).is_some_and(|value| value == "1")
}

/// Drops the owner's keyboard before any window exists, and starts the typist in a typing run. Main thread.
fn guard_the_keyboard() {
    // SAFETY: main thread; installs one local event monitor, once, before the first window.
    unsafe { ck_r2_t_install_input_filter() };
    if typing() {
        // SAFETY: main thread, after the filter; starts one thread that only posts events.
        unsafe { ck_r2_t_start_typist() };
    }
}

/// Whether the keyboard guard is satisfied: a quiet run needs nothing, a typing run needs the filter to have
/// dropped keys (so it proves something about typing).
fn typing_was_dropped() -> bool {
    // SAFETY: reads an atomic counter.
    !typing() || unsafe { ck_r2_t_dropped_keys() } > 0
}

/// Close requests that reached the worker's UI. The filter drops Escape before the window sees it, so none may.
static CLOSE_REQUESTS: AtomicUsize = AtomicUsize::new(0);

fn main() -> ExitCode {
    match env::var(WORKER_VARIABLE).as_deref() {
        Ok("basic") => worker(),
        Ok("identity") => identity_worker(),
        Ok("res-lifecycle") => resources_worker(Plan::Lifecycle),
        Ok("res-delayed-checker") => resources_worker(Plan::DelayedChecker),
        Ok("res-checker-failure") => resources_worker(Plan::CheckerFailure),
        Ok("res-early-close") => resources_worker(Plan::EarlyClose),
        Ok("res-early-close-retained") => resources_worker(Plan::EarlyCloseRetained),
        Ok("res-relay") => relay_worker(),
        Ok("res-movement-reupload") => resources_worker(Plan::MovementReupload),
        Ok("res-oracles") => oracles_worker(),
        Ok("res-compat") => compat_worker(),
        // The supervisor's own fixtures: no window, no GPU.
        Ok("probe-clean") => {
            eprintln!("a clean worker");
            ExitCode::SUCCESS
        }
        Ok("probe-invalid-utf8") => {
            // A byte that is not UTF-8, then a clean exit: an unreadable diagnostic stream with success.
            let _ = std::io::stderr().write_all(&[b'o', b'k', 0xff]);
            ExitCode::SUCCESS
        }
        Ok(other) => {
            eprintln!("unknown worker scenario {other:?}");
            ExitCode::FAILURE
        }
        Err(_) => supervisor(),
    }
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

/// The worker process: killed and reaped when this is dropped, on every path.
struct Owned(Option<Child>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// What one captured stream came to: its text, or why it could not be read as text.
type Stream = Result<String, String>;

/// Reads a pipe to its end on a thread. A read error, or bytes that are not UTF-8, is an error naming the stream:
/// an unreadable stream is never taken for an empty or a clean one.
fn read_all(
    name: &'static str,
    mut pipe: impl Read + Send + 'static,
) -> thread::JoinHandle<Stream> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Err(error) = pipe.read_to_end(&mut bytes) {
            return Err(format!(
                "{name}: the read failed after {} bytes: {error}",
                bytes.len()
            ));
        }
        String::from_utf8(bytes).map_err(|error| format!("{name}: not valid UTF-8: {error}"))
    })
}

/// The reader thread's result, with a thread that panicked or was lost counted as a failed stream.
fn joined(name: &str, handle: thread::JoinHandle<Stream>) -> Stream {
    handle
        .join()
        .unwrap_or_else(|_| Err(format!("{name}: the reader thread panicked")))
}

/// How one worker run went.
struct Outcome {
    passed: bool,
    report: String,
}

/// Judges a worker run from what was observed of it: `status` is `None` when it was killed at the deadline.
/// Every way the evidence can be missing fails the run, whatever the exit status and whatever text was read.
fn judge(scenario: &str, status: Option<ExitStatus>, stdout: &Stream, stderr: &Stream) -> Outcome {
    let mut problems = Vec::new();
    match status {
        None => problems.push("the worker did not finish by the deadline".to_owned()),
        Some(status) if !status.success() => {
            problems.push(format!("the worker exited with {status:?}"));
        }
        Some(_) => {}
    }
    for stream in [stdout, stderr] {
        if let Err(error) = stream {
            problems.push(error.clone());
        }
    }
    if let Ok(text) = stderr
        && let Some(marker) = VALIDATION_MARKERS
            .iter()
            .find(|marker| text.contains(**marker))
    {
        problems.push(format!("Metal validation reported {marker:?}"));
    }
    let passed = problems.is_empty();
    let report = if passed {
        String::new()
    } else {
        format!(
            "scenario {scenario}: {}\n--- stdout\n{}\n--- stderr\n{}",
            problems.join("; "),
            stdout.as_deref().unwrap_or("(unreadable)"),
            stderr.as_deref().unwrap_or("(unreadable)")
        )
    };
    Outcome { passed, report }
}

fn run_worker(scenario: &str, typing: bool) -> Outcome {
    let exe = env::current_exe().expect("the test binary's path");
    let mut command = Command::new(exe);
    command
        .env(WORKER_VARIABLE, scenario)
        .env(TYPING_VARIABLE, if typing { "1" } else { "0" })
        .env("MTL_DEBUG_LAYER", "1")
        .env("MTL_SHADER_VALIDATION", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the worker starts");
    let (stdout, stderr) = (
        read_all("stdout", child.stdout.take().expect("piped")),
        read_all("stderr", child.stderr.take().expect("piped")),
    );
    let mut owned = Owned(Some(child));
    let started = Instant::now();
    let status = loop {
        let child = owned.0.as_mut().expect("the worker is running");
        if let Some(status) = child.try_wait().expect("the worker can be waited for") {
            break Some(status);
        }
        if started.elapsed() >= DEADLINE {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    drop(owned); // killed (if it is still running) and reaped before the pipes are read
    let label = format!("{scenario} ({})", if typing { "typing" } else { "quiet" });
    judge(
        &label,
        status,
        &joined("stdout", stdout),
        &joined("stderr", stderr),
    )
}

/// A reader that fails after some bytes.
struct FailingReader(bool);

impl Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.0 {
            return Err(std::io::Error::other("the pipe broke"));
        }
        self.0 = true;
        buffer[..2].copy_from_slice(b"hi");
        Ok(2)
    }
}

/// The supervisor's own judgment, on fixtures: it fails closed on every missing or unreadable piece of evidence.
fn judgment_checks() -> Result<(), String> {
    let ok: Stream = Ok("fine\n".to_owned());
    let success = Some(ExitStatus::from_raw(0));
    let failure = Some(ExitStatus::from_raw(1 << 8));
    let invalid = |bytes: &[u8]| {
        joined(
            "stderr",
            read_all("stderr", std::io::Cursor::new(bytes.to_vec())),
        )
    };
    let cases: Vec<(&str, bool, Outcome)> = vec![
        ("a clean zero-exit run", true, judge("c", success, &ok, &ok)),
        (
            "invalid UTF-8 before any text",
            false,
            judge("c", success, &ok, &invalid(&[0xff])),
        ),
        (
            "invalid UTF-8 after a clean prefix",
            false,
            judge("c", success, &ok, &invalid(b"fine so far\n\xff")),
        ),
        (
            "a read error after some bytes",
            false,
            judge(
                "c",
                success,
                &ok,
                &joined("stderr", read_all("stderr", FailingReader(false))),
            ),
        ),
        (
            "an unreadable stdout",
            false,
            judge("c", success, &invalid(&[0xc3, 0x28]), &ok),
        ),
        ("a nonzero exit", false, judge("c", failure, &ok, &ok)),
        (
            "a run killed at the deadline",
            false,
            judge("c", None, &ok, &ok),
        ),
        (
            "a validation marker on stderr",
            false,
            judge("c", success, &ok, &Ok("MTLDebug: bad".to_owned())),
        ),
    ];
    for (what, expected, outcome) in cases {
        if outcome.passed != expected {
            return Err(format!(
                "judgment: {what}: passed={}, expected {expected}",
                outcome.passed
            ));
        }
    }
    // And through real child processes and pipes: the same judgment path the workers use.
    if !run_worker("probe-clean", false).passed {
        return Err("judgment: the clean control child did not pass".to_owned());
    }
    let unreadable = run_worker("probe-invalid-utf8", false);
    if unreadable.passed || !unreadable.report.contains("stderr: not valid UTF-8") {
        return Err(format!(
            "judgment: a zero-exit child with an unreadable stderr must fail naming the stream: {}",
            unreadable.report
        ));
    }
    Ok(())
}

/// The keyboard filter lives in this test's archive and in no library artifact: the archive has the filter and
/// the typist, and no `libck_render_2d` artifact built beside this executable has either.
fn input_filter_isolation() -> Result<(), String> {
    let archive = std::fs::read(env!("CK_RENDER_2D_TESTING_ARCHIVE"))
        .map_err(|error| format!("isolation: the test support archive cannot be read: {error}"))?;
    let has = |haystack: &[u8], needle: &str| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };
    for name in [
        "ck_r2_t_install_input_filter",
        "ck_r2_t_start_typist",
        "ck_r2_t_dropped_keys",
    ] {
        if !has(&archive, name) {
            return Err(format!("isolation: the test support archive lacks {name}"));
        }
    }
    let exe = env::current_exe().map_err(|error| format!("isolation: {error}"))?;
    let directory = exe
        .parent()
        .ok_or("isolation: the test executable has no directory")?;
    let mut libraries = 0;
    for entry in std::fs::read_dir(directory).map_err(|error| format!("isolation: {error}"))? {
        let path = entry.map_err(|error| format!("isolation: {error}"))?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name.starts_with("libck_render_2d-")
            && Path::new(name).extension().is_some_and(|e| e == "rlib")
        {
            libraries += 1;
            let bytes =
                std::fs::read(&path).map_err(|error| format!("isolation: {name}: {error}"))?;
            if has(&bytes, "ck_r2_t_") {
                return Err(format!(
                    "isolation: {name} contains test support (ck_r2_t_)"
                ));
            }
        }
    }
    if libraries == 0 {
        return Err("isolation: no libck_render_2d artifact was found to check".to_owned());
    }
    Ok(())
}

fn supervisor() -> ExitCode {
    let started = Instant::now();
    let mut failures: Vec<String> = [judgment_checks(), input_filter_isolation()]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    if failures.is_empty() {
        let outcomes: Vec<Outcome> = SCENARIOS
            .iter()
            .flat_map(|scenario| [false, true].map(|typing| run_worker(scenario, typing)))
            .collect();
        failures.extend(
            outcomes
                .into_iter()
                .filter(|outcome| !outcome.passed)
                .map(|outcome| outcome.report),
        );
    }
    let passed = failures.is_empty();
    println!(
        "test metal_window ... {} ({:.1} s)",
        if passed { "ok" } else { "FAILED" },
        started.elapsed().as_secs_f64()
    );
    if !passed {
        for failure in failures {
            println!("{failure}");
        }
        return ExitCode::FAILURE;
    }
    println!("test result: ok. 1 passed; 0 failed");
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------------------------

/// What the main thread saw of the presenter, for the scripted UI.
#[derive(Default)]
struct Probe {
    drawn: AtomicU64,
    any: AtomicBool,
    /// Every distinct generation seen drawn, in order.
    seen: Mutex<Vec<u64>>,
}

impl Probe {
    fn record(&self, generation: Option<u64>) {
        let Some(generation) = generation else { return };
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.last() != Some(&generation) {
            seen.push(generation);
        }
        self.drawn.store(generation, Ordering::Release);
        self.any.store(true, Ordering::Release);
    }

    fn drawn(&self) -> Option<u64> {
        self.any
            .load(Ordering::Acquire)
            .then(|| self.drawn.load(Ordering::Acquire))
    }
}

/// `MetalBackend` with a look at `drawn_generation` after every report poll and every end poll (both run on the
/// main thread, each pass of the loop).
struct Spy {
    inner: MetalBackend,
    probe: Arc<Probe>,
}

impl Gpu2d for Spy {
    type Handles = CocoaHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.inner.new_sink()
    }

    fn start(
        &mut self,
        handles: CocoaHandles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.inner.start(handles, desc, pixel_size, sink)
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.inner.set_clear(rgb);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.inner.set_pixel_size(width, height);
    }

    fn request_redraw(&mut self) {
        self.inner.request_redraw();
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.probe.record(self.inner.drawn_generation());
        self.inner.take_report()
    }

    fn request_stop(&mut self) {
        self.inner.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.probe.record(self.inner.drawn_generation());
        self.inner.poll_end(within)
    }
}

/// Where the script is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waiting for the first frame.
    First,
    /// The quad was cleared; waiting for the frame of that scene.
    Cleared,
    /// The window was asked to close.
    Closing,
}

/// A fake UI VM: it drives the scene and the window from what the presenter drew, and declines every close
/// request (a real Escape from a typing owner must change nothing).
struct Script {
    probe: Arc<Probe>,
    scene: SceneWriter,
    requests: RequestWriter,
    stage: Stage,
    opened: bool,
    answers: Vec<UiAnswer>,
}

impl UiLink for Script {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => self.opened = true,
            UiEvent::CloseRequested { .. } => {
                CLOSE_REQUESTS.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => panic!("unexpected UI update in test script: {update:?}"),
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        match (self.stage, self.probe.drawn()) {
            (Stage::First, Some(FIRST)) if self.opened => {
                // The first frame has the quad written before the window opened. Clear it: the next scene
                // shows nothing, with its own generation.
                self.scene.clear_quad();
                self.stage = Stage::Cleared;
            }
            (Stage::Cleared, Some(generation))
                if generation == FIRST + 1 && typing_was_dropped() =>
            {
                self.requests.close();
                self.stage = Stage::Closing;
            }
            _ => {}
        }
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

/// A hang in the loop ends the process, so the supervisor reports it.
fn watchdog() {
    thread::spawn(|| {
        thread::sleep(DEADLINE - Duration::from_secs(10));
        eprintln!("worker: the window loop did not finish in time");
        std::process::exit(2);
    });
}

/// Frames after the first produce no report, so nothing would wake an idle loop: a ticker does (a test-only
/// stimulus). Returns the flag that stops it and its thread.
fn start_ticker(notifier: &Arc<Notifier>) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let running = Arc::new(AtomicBool::new(true));
    let ticker = {
        let (notifier, running) = (Arc::clone(notifier), Arc::clone(&running));
        thread::spawn(move || {
            while running.load(Ordering::Acquire) {
                notifier.wake();
                thread::sleep(Duration::from_millis(5));
            }
        })
    };
    (running, ticker)
}

fn worker() -> ExitCode {
    ck_window::test_support::open_windows_without_activation();
    activation_watch::install(false);
    watchdog();
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: "Codekarl render-2d Metal window test".to_owned(),
            width: 200,
            height: 150,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    let bridge = SceneBridge::new(Filter::Nearest);
    let scene = bridge.take_writer().expect("the one writer");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rgba8.png");
    let image =
        Arc::new(Image::decode(std::fs::File::open(path).expect("the fixture")).expect("a PNG"));
    // Before the window exists: generation 1.
    scene.set_quad(Quad::new(image, 10.0, 10.0, Some(40.0), Some(40.0)).expect("a valid quad"));

    let probe = Arc::new(Probe::default());
    let wake = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    // SAFETY: `ck_window::run` below keeps the window (and so its layer) until the presenter's end is proven,
    // which is the contract of `MetalBackend::new`; the backend is used as nothing but that presenter.
    let backend = unsafe { MetalBackend::new(wake) };
    let renderer = Renderer::new(
        Spy {
            inner: backend,
            probe: Arc::clone(&probe),
        },
        Arc::clone(&bridge),
    );
    let ui = Script {
        probe: Arc::clone(&probe),
        scene,
        requests: writer,
        stage: Stage::First,
        opened: false,
        answers: Vec::new(),
    };
    let cocoa = match CocoaBackend::new() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            return ExitCode::FAILURE;
        }
    };
    guard_the_keyboard();
    let (running, ticker) = start_ticker(&notifier);
    let quit = AtomicBool::new(false);
    let end = run(
        cocoa,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = ticker.join();

    let seen = probe
        .seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let problems = check(&end, &seen, &bridge);
    if problems.is_empty() {
        activation_watch::assert_clean();
        println!("worker: drawn generations {seen:?}; the loop ended {end:?}");
        ExitCode::SUCCESS
    } else {
        for problem in problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}

/// What is wrong with how the run went, if anything.
fn check(end: &LoopEnd, seen: &[u64], bridge: &SceneBridge) -> Vec<String> {
    let mut problems = Vec::new();
    if *end != LoopEnd::Closed {
        problems.push(format!(
            "the loop ended with {end:?}, not Closed (a proven teardown)"
        ));
    }
    if seen != [FIRST, FIRST + 1] {
        problems.push(format!(
            "the drawn generations were {seen:?}; expected [{FIRST}, {}]: the quad written before the window opened in the first frame, then the scene without a quad",
            FIRST + 1
        ));
    }
    let closes = CLOSE_REQUESTS.load(Ordering::SeqCst);
    if closes != 0 {
        problems.push(format!(
            "{closes} close request(s) reached the UI: the keyboard filter let Escape through"
        ));
    }
    let counts = bridge.counts();
    if counts
        != (WriteCounts {
            accepted: 2,
            discarded: 0,
            published: 0,
            publication_discarded: 0,
        })
    {
        problems.push(format!("the scene writes were {counts:?}"));
    }
    if bridge.phase() != Phase::Ended {
        problems.push(format!("the bridge is {:?}, not Ended", bridge.phase()));
    }
    problems
}

// ---------------------------------------------------------------------------------------------
// The source identity scenario
// ---------------------------------------------------------------------------------------------

/// The real presenter, started with the backend's sink conversion and the presenter's observer, so the test can
/// count the textures the render thread uploaded. Test-only: `MetalBackend` does not offer the observer.
struct ObservedBackend {
    wake: Arc<dyn Fn() + Send + Sync>,
    slot: Arc<MetalQuadSlot>,
    observer: Arc<Observer>,
    presenter: Option<MetalPresenter>,
    probe: Arc<Probe>,
}

impl Gpu2d for ObservedBackend {
    type Handles = CocoaHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        sink_over_slot(Arc::clone(&self.slot), Arc::clone(&self.wake))
    }

    fn start(
        &mut self,
        handles: CocoaHandles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        _sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        let hooks = TestHooks {
            quads: Some(Arc::clone(&self.slot)),
            observer: Some(Arc::clone(&self.observer)),
            ..TestHooks::default()
        };
        // SAFETY: `handles.layer()` is the live `CAMetalLayer` of the window `ck_window::run` holds until the
        // presenter's end is proven, as `start_with_test_hooks` requires.
        let started = unsafe {
            MetalPresenter::start_with_test_hooks(
                handles.layer(),
                (pixel_size.width, pixel_size.height),
                desc.clear,
                Arc::clone(&self.wake),
                hooks,
            )
        };
        self.presenter = Some(started.map_err(|error| error.to_string())?);
        Ok(())
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        if let Some(presenter) = &self.presenter {
            presenter.set_clear(rgb);
        }
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        if let Some(presenter) = &self.presenter {
            presenter.set_pixel_size(width, height);
        }
    }

    fn request_redraw(&mut self) {
        if let Some(presenter) = &self.presenter {
            presenter.request_redraw();
        }
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        let presenter = self.presenter.as_ref()?;
        self.probe.record(presenter.drawn_generation());
        presenter.take_report().map(|report| match report {
            GpuReport::FirstFrameQueued => PresenterReport::FirstFrameQueued,
            GpuReport::Failed { message } => PresenterReport::Failed { message },
        })
    }

    fn request_stop(&mut self) {
        if let Some(presenter) = &self.presenter {
            presenter.request_stop();
        }
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        let presenter = self.presenter.as_mut()?;
        self.probe.record(presenter.drawn_generation());
        presenter.poll_end(within).map(|result| match result {
            GpuCleanup::Proven => CleanupResult::Proven,
            GpuCleanup::Unproven { reason } => CleanupResult::Unproven { reason },
        })
    }
}

/// The identity script's steps, each started by the generation the previous one produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Waiting for the first frame (the quad written before the window opened).
    First,
    /// The same image was republished with a moved quad.
    Moved,
    /// A new image was published.
    Replaced,
    /// The window was asked to close.
    Closing,
}

struct IdentityScript {
    probe: Arc<Probe>,
    observer: Arc<Observer>,
    scene: SceneWriter,
    requests: RequestWriter,
    image: Arc<Image>,
    other: Arc<Image>,
    step: Step,
    opened: bool,
    answers: Vec<UiAnswer>,
    /// The textures uploaded so far, read when each step's frame was drawn (shared out of the loop).
    uploads: Arc<Mutex<Vec<i64>>>,
}

impl IdentityScript {
    fn record_uploads(&self) {
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.observer.quad_textures_created());
    }
}

impl UiLink for IdentityScript {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => self.opened = true,
            UiEvent::CloseRequested { .. } => {
                CLOSE_REQUESTS.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => {
                panic!("unexpected UI update in identity script: {update:?}")
            }
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        let drawn = self.probe.drawn();
        match (self.step, drawn) {
            (Step::First, Some(FIRST)) if self.opened => {
                self.record_uploads();
                let moved = Quad::new(Arc::clone(&self.image), 80.0, 60.0, Some(40.0), Some(40.0));
                self.scene.set_quad(moved.expect("a valid quad"));
                self.step = Step::Moved;
            }
            (Step::Moved, Some(generation)) if generation == FIRST + 1 => {
                self.record_uploads();
                let replaced =
                    Quad::new(Arc::clone(&self.other), 10.0, 10.0, Some(40.0), Some(40.0));
                self.scene.set_quad(replaced.expect("a valid quad"));
                self.step = Step::Replaced;
            }
            (Step::Replaced, Some(generation))
                if generation == FIRST + 2 && typing_was_dropped() =>
            {
                self.record_uploads();
                self.requests.close();
                self.step = Step::Closing;
            }
            _ => {}
        }
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

fn identity_worker() -> ExitCode {
    watchdog();
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: "Codekarl render-2d source identity test".to_owned(),
            width: 200,
            height: 150,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    let bridge = SceneBridge::new(Filter::Nearest);
    let scene = bridge.take_writer().expect("the one writer");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rgba8.png");
    let decode =
        || Arc::new(Image::decode(std::fs::File::open(path).expect("the fixture")).expect("a PNG"));
    let (image, other) = (decode(), decode());
    scene.set_quad(
        Quad::new(Arc::clone(&image), 10.0, 10.0, Some(40.0), Some(40.0)).expect("a valid quad"),
    );

    let probe = Arc::new(Probe::default());
    let observer = Arc::new(Observer::new());
    let wake = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    let renderer = Renderer::new(
        ObservedBackend {
            wake,
            slot: MetalQuadSlot::new(),
            observer: Arc::clone(&observer),
            presenter: None,
            probe: Arc::clone(&probe),
        },
        Arc::clone(&bridge),
    );
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let ui = IdentityScript {
        probe: Arc::clone(&probe),
        observer,
        scene,
        requests: writer,
        image,
        other,
        step: Step::First,
        opened: false,
        answers: Vec::new(),
        uploads: Arc::clone(&uploads),
    };
    let cocoa = match CocoaBackend::new() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            return ExitCode::FAILURE;
        }
    };
    guard_the_keyboard();
    let (running, ticker) = start_ticker(&notifier);
    let quit = AtomicBool::new(false);
    let end = run(
        cocoa,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = ticker.join();

    let seen = probe
        .seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let uploads = uploads
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let problems = check_identity(&end, &seen, &uploads, &bridge);
    if problems.is_empty() {
        println!("worker: drawn {seen:?}; uploads {uploads:?}; the loop ended {end:?}");
        ExitCode::SUCCESS
    } else {
        for problem in problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}

/// What is wrong with how the identity run went, if anything.
fn check_identity(
    end: &LoopEnd,
    seen: &[u64],
    uploads: &[i64],
    bridge: &SceneBridge,
) -> Vec<String> {
    let mut problems = Vec::new();
    if *end != LoopEnd::Closed {
        problems.push(format!("the loop ended with {end:?}, not Closed"));
    }
    if seen != [FIRST, FIRST + 1, FIRST + 2] {
        problems.push(format!("the drawn generations were {seen:?}"));
    }
    // One upload for the first frame; none more for the same image with a moved quad; one more for a new image.
    if uploads != [1, 1, 2] {
        problems.push(format!(
            "textures uploaded after each frame were {uploads:?}; expected [1, 1, 2]"
        ));
    }
    let closes = CLOSE_REQUESTS.load(Ordering::SeqCst);
    if closes != 0 {
        problems.push(format!(
            "{closes} close request(s) reached the UI: the keyboard filter let Escape through"
        ));
    }
    if bridge.phase() != Phase::Ended {
        problems.push(format!("the bridge is {:?}, not Ended", bridge.phase()));
    }
    problems
}

// ---------------------------------------------------------------------------------------------
// The resource scenarios (CK-117): `MetalBackend::with_resources` on a real window
// ---------------------------------------------------------------------------------------------

/// What a resource run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Textures and quads before the window opens with their jobs gated: the first frame shows the checker for each quad;
    /// then the jobs run (ready), the quads move (no upload), one texture is reloaded, one released, the window closed.
    Lifecycle,
    /// The same first frame with the checker's own upload held (and the jobs gated): still no frame until it is resident.
    DelayedChecker,
    /// The checker's upload fails: one presenter `Failed`, no frame.
    CheckerFailure,
    /// The window is closed before the first frame, the checker's upload released only after the stop: proven, no frame.
    EarlyClose,
    /// The same with the upload never released and a short stop budget: retained, no frame.
    EarlyCloseRetained,
    /// The negative control of the movement check: one texture is uploaded again (same pixels) while the quads move, and the
    /// cumulative upload counters, compared **without** the revisions, must show it.
    MovementReupload,
}

impl Plan {
    fn name(self) -> &'static str {
        match self {
            Self::Lifecycle => "res-lifecycle",
            Self::DelayedChecker => "res-delayed-checker",
            Self::CheckerFailure => "res-checker-failure",
            Self::EarlyClose => "res-early-close",
            Self::EarlyCloseRetained => "res-early-close-retained",
            Self::MovementReupload => "res-movement-reupload",
        }
    }
}

/// The side of 8 x 8 solid textures the scenarios use.
const SIDE: u32 = 8;

fn solid(service: &ResourceService, colour: [u8; 4]) -> CpuPixels {
    let reservation = service
        .reserve_pixels(SIDE, SIDE)
        .expect("a pixel reservation");
    let bytes = colour.repeat((SIDE * SIDE) as usize);
    CpuPixels::premultiplied(reservation, bytes).expect("RGBA8 of the right length")
}

/// What the main thread saw of a resource run.
#[derive(Default)]
struct ResProbe {
    drawn: Probe,
    /// The presenter's latest counters, read after every report poll and every end poll.
    stats: Mutex<Option<SceneStats>>,
    /// Every way the contract was seen broken, as it happened.
    violations: Mutex<Vec<String>>,
    /// `Failed` reports taken.
    failures: AtomicUsize,
    /// The backend was asked to stop.
    stop_requested: AtomicBool,
}

impl ResProbe {
    fn stats(&self) -> Option<SceneStats> {
        *self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn violate(&self, message: String) {
        self.violations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(message);
    }
}

/// `MetalBackend::with_resources` with a look at the presenter's counters, on the main thread, after every report poll
/// and every end poll.
struct ResSpy {
    inner: MetalBackend,
    probe: Arc<ResProbe>,
    observer: Arc<Observer>,
}

impl ResSpy {
    fn observe(&self) {
        // The commit count is read before the counters: a frame committed implies the checker was resident, and
        // residency never goes away, so the later read cannot see "not resident" after a commit.
        let committed = self.observer.committed();
        let stats = self.inner.scene_stats();
        if let Some(stats) = stats {
            if committed > 0 && !stats.checker_resident {
                self.probe.violate(format!(
                    "{committed} frame(s) committed before the checker was resident"
                ));
            }
            *self
                .probe
                .stats
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(stats);
        }
        self.probe.drawn.record(self.inner.drawn_generation());
    }
}

impl Gpu2d for ResSpy {
    type Handles = CocoaHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.inner.new_sink()
    }

    fn start(
        &mut self,
        handles: CocoaHandles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.inner.start(handles, desc, pixel_size, sink)
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.inner.set_clear(rgb);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.inner.set_pixel_size(width, height);
    }

    fn request_redraw(&mut self) {
        self.inner.request_redraw();
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.observe();
        let report = self.inner.take_report();
        if matches!(report, Some(PresenterReport::Failed { .. })) {
            self.probe.failures.fetch_add(1, Ordering::SeqCst);
        }
        report
    }

    fn request_stop(&mut self) {
        self.probe.stop_requested.store(true, Ordering::SeqCst);
        self.inner.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.observe();
        self.inner.poll_end(within)
    }
}

/// Where a resource run's script is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResStage {
    /// Before the window has opened and the first frame was seen (or the checker was released).
    First,
    /// The jobs ran; waiting for both textures to be ready and drawn as resolved.
    Ready,
    /// Both ready and drawn resolved at this frame count; waiting for one more frame, so every upload counter of the
    /// uploads that made them ready has been published (the counters are published with each texture pass).
    Settling(u64),
    /// Quads are moving; `usize` moves were made.
    Moving(usize),
    /// A texture was reloaded; waiting for its second revision to be shown.
    Reloaded,
    /// A quad was destroyed and its texture released; waiting for the texture to retire.
    Released,
    /// The control's measurement is done: only the close is left, retried until the typist has had a key dropped (the
    /// baseline was consumed, so nothing else may run).
    Finishing,
    /// The window was asked to close.
    Closing,
}

/// The resources and quads of a resource run, and what the script measured at each step.
struct ResScript {
    probe: Arc<ResProbe>,
    observer: Arc<Observer>,
    plan: Plan,
    service: ResourceService,
    context: Arc<MetalContext>,
    scene: SceneWriter,
    requests: RequestWriter,
    pool_slot: Arc<Mutex<Option<LoaderPool>>>,
    /// The gate of the checker's upload (the held scenarios).
    checker_gate: Option<Arc<AtomicI32>>,
    textures: [TextureKey; 2],
    quads: [QuadKey; 2],
    stage: ResStage,
    opened: bool,
    /// Passes of the loop seen after it opened (for the "nothing happens while the checker is held" phase).
    passes: usize,
    baseline: Option<Baseline>,
    /// The frames drawn when the movement ended.
    moved_frames: u64,
    /// The frames drawn when the quad was destroyed and its texture released: the release step needs a later one.
    release_frames: u64,
    /// The control uploaded again.
    reuploaded: bool,
    /// Loop passes for which the close is held back as if no key had been dropped yet (the control only).
    close_delay: usize,
    answers: Vec<UiAnswer>,
}

/// What the movement step compares against.
struct Baseline {
    frames: u64,
    work: Work,
    requested: [u64; 2],
    shown: [Option<u64>; 2],
}

/// What the upload store did over its life (cumulative, from the presenter's scene stats) and how many Metal textures are
/// alive right now. Moving quads must change none of it: occupancy alone would miss a texture destroyed and created again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Work {
    textures_created: u64,
    uploads_submitted: u64,
    uploads_completed: u64,
    live_textures: u64,
}

impl Work {
    fn of(stats: &SceneStats, live_textures: u64) -> Self {
        Self {
            textures_created: stats.textures_created,
            uploads_submitted: stats.uploads_submitted,
            uploads_completed: stats.uploads_completed,
            live_textures,
        }
    }
}

/// What differs between two readings of [`Work`], for a phase that must upload and create nothing.
fn movement_work_problems(before: Work, after: Work) -> Vec<String> {
    let mut problems = Vec::new();
    for (what, was, is) in [
        (
            "textures created",
            before.textures_created,
            after.textures_created,
        ),
        (
            "upload batches submitted",
            before.uploads_submitted,
            after.uploads_submitted,
        ),
        (
            "upload batches completed",
            before.uploads_completed,
            after.uploads_completed,
        ),
        (
            "live Metal textures",
            before.live_textures,
            after.live_textures,
        ),
    ] {
        if was != is {
            problems.push(format!("moving quads changed the {what}: {was} then {is}"));
        }
    }
    problems
}

/// Whether the requested or shown revision of either texture differs between two readings.
fn revisions_moved(
    before: ([u64; 2], [Option<u64>; 2]),
    now: ([u64; 2], [Option<u64>; 2]),
) -> bool {
    before != now
}

/// Whether the release step's frame bound is from before the movement ended (a stale bound). Equal is valid: the reload's
/// shown acknowledgement precedes the frame that commits it, so both sampled counts can be the same.
fn release_bound_is_stale(release_frames: u64, moved_frames: u64) -> bool {
    release_frames < moved_frames
}

/// Whether the release phase is over: the released key retired (one live entry, none retiring) **and** a frame was committed
/// after the release whose scene is the surviving quad alone, resolved. `release_frames` is the frame count when the quad was
/// destroyed, so no frame drawn before it can satisfy this.
fn release_is_complete(live: u32, retiring: u32, stats: &SceneStats, release_frames: u64) -> bool {
    live == 1
        && retiring == 0
        && stats.frames > release_frames
        && (
            stats.last.instances,
            stats.last.resolved,
            stats.last.checker,
        ) == (1, 1, 0)
}

impl ResScript {
    fn stats(&self) -> Option<SceneStats> {
        self.probe.stats()
    }

    fn revision(&self, index: usize) -> (u64, Option<u64>, StatusState) {
        let status = self
            .service
            .status(self.textures[index])
            .expect("a live texture");
        (
            status.requested,
            status.shown.map(|shown| shown.revision),
            status.state,
        )
    }

    fn violate(&self, message: &str) {
        self.probe.violate(message.to_owned());
    }

    fn first_frame_is_the_checker(&self, stats: &SceneStats) {
        if (
            stats.last.instances,
            stats.last.checker,
            stats.last.resolved,
        ) != (2, 2, 0)
        {
            self.violate(&format!(
                "the first frame should show the checker for both quads, with no resolved instance: {:?}",
                stats.last
            ));
        }
    }

    fn step(&mut self) {
        match self.plan {
            Plan::Lifecycle | Plan::MovementReupload => self.step_lifecycle(),
            Plan::DelayedChecker => self.step_delayed_checker(),
            Plan::EarlyClose | Plan::EarlyCloseRetained => self.step_early_close(),
            // Nothing to drive: the run ends by the presenter's failure.
            Plan::CheckerFailure => {}
        }
    }

    fn step_delayed_checker(&mut self) {
        // The loop posts `Opened` to the UI after the first frame, which the held checker prevents: the phase is counted
        // from the presenter's start instead.
        if self.stats().is_some() {
            self.passes += 1;
        }
        let held = self.stats().is_some_and(|stats| !stats.checker_resident);
        if self.checker_gate.is_some() && self.passes >= 40 {
            // Many loop passes after the presenter started: the held checker means no frame has been committed.
            if !held || self.observer.committed() != 0 {
                self.violate(&format!(
                    "a frame was committed while the checker's upload was held (held {held}, committed {}, stats {:?})",
                    self.observer.committed(),
                    self.stats()
                ));
            }
            if let Some(gate) = self.checker_gate.take() {
                gate.store(1, Ordering::SeqCst);
            }
        }
        if self.checker_gate.is_none()
            && let Some(stats) = self.stats().filter(|stats| stats.frames >= 1)
        {
            self.first_frame_is_the_checker(&stats);
            self.close_when_keys_were_dropped();
        }
    }

    fn step_early_close(&mut self) {
        // The presenter is started and the checker's upload is held: close before any frame.
        if self.stats().is_some() {
            self.passes += 1;
        }
        if self.passes >= 5 {
            self.requests.close();
            self.stage = ResStage::Closing;
        }
    }

    fn step_lifecycle(&mut self) {
        match self.stage {
            ResStage::First => {
                if let Some(stats) = self
                    .stats()
                    .filter(|stats| stats.frames >= 1 && self.opened)
                {
                    self.first_frame_is_the_checker(&stats);
                    // The jobs were gated until now: the window opened and drew without them.
                    let pool = LoaderPool::start(self.service.clone()).expect("a loader pool");
                    *self
                        .pool_slot
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(pool);
                    self.stage = ResStage::Ready;
                }
            }
            ResStage::Ready => {
                let ready = [0, 1]
                    .iter()
                    .all(|&i| self.revision(i).2 == StatusState::Ready);
                if let Some(stats) = self
                    .stats()
                    .filter(|stats| ready && stats.last.resolved == 2)
                {
                    if stats.last.checker != 0 {
                        self.violate("a resolved frame still shows a checker");
                    }
                    // Nothing else makes the presenter draw another frame once the textures are resident: a small move
                    // (before the baseline is taken) asks for one.
                    self.scene
                        .set_position(self.quads[0], 11.0, 10.0)
                        .expect("a live quad moves");
                    self.scene.publish();
                    self.stage = ResStage::Settling(stats.frames);
                }
            }
            ResStage::Settling(frames) => {
                if let Some(stats) = self.stats().filter(|stats| stats.frames > frames) {
                    self.baseline = Some(Baseline {
                        frames: stats.frames,
                        work: self.work_now(&stats),
                        requested: [self.revision(0).0, self.revision(1).0],
                        shown: [self.revision(0).1, self.revision(1).1],
                    });
                    self.stage = ResStage::Moving(0);
                }
            }
            ResStage::Moving(moves) => self.move_quads(moves),
            ResStage::Reloaded => {
                let (requested, shown, state) = self.revision(0);
                if requested == 2 && shown == Some(2) && state == StatusState::Ready {
                    if self.scene.quad_count() != 2 {
                        self.violate("the reload changed the quads");
                    }
                    // Release: destroy quad 0, drop its texture's token. The release step needs a frame after this one.
                    self.release_frames = self.stats().map_or(0, |stats| stats.frames);
                    self.scene.destroy_quad(self.quads[0]).expect("a live quad");
                    self.scene.publish();
                    self.service
                        .release(self.textures[0])
                        .expect("a live texture");
                    self.stage = ResStage::Released;
                }
            }
            ResStage::Released => {
                let registry = self.service.stats().registry;
                if let Some(stats) = self.stats()
                    && release_is_complete(
                        registry.live,
                        registry.retiring,
                        &stats,
                        self.release_frames,
                    )
                {
                    // The bound was taken at the release, after the movement and the reload: not the movement's own.
                    // Equal is valid: the shown acknowledgement of the reload precedes the frame that commits it, so the
                    // two sampled counts can be the same; only a bound from before the movement is wrong.
                    if release_bound_is_stale(self.release_frames, self.moved_frames) {
                        self.violate(&format!(
                            "the release step's frame bound {} is before the movement's end at {}",
                            self.release_frames, self.moved_frames
                        ));
                    }
                    self.close_when_keys_were_dropped();
                }
            }
            ResStage::Finishing => self.close_when_keys_were_dropped(),
            ResStage::Closing => {}
        }
    }

    /// The cumulative work and live textures now.
    fn work_now(&self, stats: &SceneStats) -> Work {
        Work::of(stats, self.context.native_live_objects().textures)
    }

    /// One move of every quad (and its publication); after enough moves and frames, the movement is over.
    fn move_quads(&mut self, moves: usize) {
        let shift = f64::from(u32::try_from(moves % 40).unwrap_or(0));
        for (index, key) in self.quads.into_iter().enumerate() {
            let offset = f64::from(u32::try_from(index).unwrap_or(0)) * 60.0;
            self.scene
                .set_position(key, 10.0 + shift + offset, 10.0 + shift)
                .expect("a live quad moves");
        }
        self.scene.publish();
        self.stage = ResStage::Moving(moves + 1);
        if self.plan == Plan::MovementReupload && !self.reuploaded && moves == 3 {
            // The control: the same pixels again (a redundant upload), while the quads keep moving.
            self.reuploaded = true;
            let pixels = solid(&self.service, [20, 200, 20, 255]);
            if self
                .service
                .replace_pixels(self.textures[0], pixels)
                .is_err()
            {
                self.violate("the control's replace_pixels was refused");
            }
        }
        let baseline = self.baseline.as_ref().expect("measured before moving");
        let reupload_shown = self.plan != Plan::MovementReupload || self.revision(0).1 == Some(2);
        if moves + 1 >= 40
            && reupload_shown
            && self
                .stats()
                .is_some_and(|stats| stats.frames >= baseline.frames + 15)
        {
            self.moved();
        }
    }

    /// The movement is over: no upload, no texture creation, no new revision. For the control, the cumulative counters
    /// alone (the revision legitimately changed) must show the redundant upload.
    fn moved(&mut self) {
        let baseline = self.baseline.take().expect("measured before moving");
        let Some(stats) = self.stats() else {
            self.violate("no presenter stats at the end of the movement");
            return;
        };
        self.moved_frames = stats.frames;
        let problems = movement_work_problems(baseline.work, self.work_now(&stats));
        if self.plan == Plan::MovementReupload {
            eprintln!("control: the cumulative counters saw: {problems:?}");
            if !problems
                .iter()
                .any(|problem| problem.contains("textures created"))
                || !problems
                    .iter()
                    .any(|problem| problem.contains("upload batches submitted"))
            {
                self.violate(&format!(
                    "the control's redundant upload was not seen by the cumulative counters: {problems:?} (work {:?} then {:?})",
                    baseline.work,
                    self.work_now(&stats)
                ));
            }
            self.stage = ResStage::Finishing;
            self.close_when_keys_were_dropped();
            return;
        }
        let mut failed = !problems.is_empty();
        for problem in problems {
            self.violate(&problem);
        }
        let now = [self.revision(0), self.revision(1)];
        if revisions_moved(
            (baseline.requested, baseline.shown),
            ([now[0].0, now[1].0], [now[0].1, now[1].1]),
        ) {
            self.violate("moving quads produced a new revision");
            failed = true;
        }
        if failed {
            // The reload and release steps below expect an untouched scene: end the run now, so the violation is the
            // failure and not a watchdog.
            self.stage = ResStage::Finishing;
            return;
        }
        // Reload one texture: a new revision, adopted at a frame boundary, with no change to the quad.
        let pixels = solid(&self.service, [200, 10, 10, 255]);
        match self.service.replace_pixels(self.textures[0], pixels) {
            Ok(2) => {}
            Ok(other) => self.violate(&format!("replace_pixels gave revision {other}, not 2")),
            Err(_) => self.violate("replace_pixels was refused"),
        }
        self.stage = ResStage::Reloaded;
    }

    fn close_when_keys_were_dropped(&mut self) {
        // The control stands for a typist whose first event is delayed: for this many passes the keyboard has dropped
        // nothing, counted in loop passes (never in time), and the script must neither panic nor close early.
        if self.close_delay > 0 {
            self.close_delay -= 1;
            return;
        }
        if typing_was_dropped() {
            self.requests.close();
            self.stage = ResStage::Closing;
        }
    }
}

impl UiLink for ResScript {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => self.opened = true,
            UiEvent::CloseRequested { .. } => {
                CLOSE_REQUESTS.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => panic!("unexpected UI update in test script: {update:?}"),
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        self.step();
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

/// Whether the relay delivers nothing to a presenter any more: fires a service wake and connects a counting wake; a
/// disconnected relay held the wake pending and delivers it once on `connect`, a connected one called its old target.
fn relay_is_disconnected(relay: &WakeRelay) -> bool {
    let delivered = Arc::new(AtomicUsize::new(0));
    (relay.wake())();
    let counter = Arc::clone(&delivered);
    relay.connect(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    relay.disconnect();
    delivered.load(Ordering::SeqCst) == 1
}

fn limits() -> ResourceBackendLimits {
    ResourceBackendLimits {
        texture: TextureLimits::default(),
        ledger: LedgerLimits::default(),
        max_quads: 1024,
        binding_geometry_bytes: 128 * 1024 * 1024,
        // The table's limits; the pre-bindless cases draw through the hidden ordinary baseline (`ordinary_baseline` of their hooks).
        bindless: BindlessLimits {
            texture_slots: 4096,
            max_textures: 1024,
            max_extra_revisions: 1024,
        },
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one scenario's setup, run and ordered teardown read best together"
)]
fn resources_worker(plan: Plan) -> ExitCode {
    watchdog();
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: "Codekarl render-2d resources test".to_owned(),
            width: 200,
            height: 150,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    // The service, with the relay's wake for what the backend must hear, and two CPU-source textures whose jobs wait for
    // a loader pool that is not started: the window opens and draws without them.
    let relay = WakeRelay::default();
    let wakes = ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let service = ResourceService::with_wakes(ResourceLimits::DEFAULT, Mode::Backend, wakes)
        .expect("a backend-mode service");
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut scene = bridge.take_writer().expect("the one writer");
    let textures = [
        service
            .create_from_pixels(solid(&service, [20, 200, 20, 255]))
            .unwrap_or_else(|_| panic!("texture 0 admitted")),
        service
            .create_from_pixels(solid(&service, [20, 20, 200, 255]))
            .unwrap_or_else(|_| panic!("texture 1 admitted")),
    ];
    let quads = [0, 1].map(|index| {
        let reference = service.retain(textures[index]).expect("a live texture");
        let x = 10.0 + 60.0 * f64::from(u32::try_from(index).unwrap_or(0));
        scene
            .create_quad(reference, x, 10.0, 40.0, 40.0)
            .expect("a quad")
    });
    // Before the window exists: the scene is published to the sink at the attach.
    assert!(scene.publish());
    // A service wake before the presenter exists stays pending in the relay until the backend connects it.
    (relay.wake())();

    let context = MetalContext::new().expect("a Metal device");
    let probe = Arc::new(ResProbe::default());
    let observer = Arc::new(Observer::new());
    let checker_gate = matches!(
        plan,
        Plan::DelayedChecker | Plan::EarlyClose | Plan::EarlyCloseRetained
    )
    .then(|| Arc::new(AtomicI32::new(0)));
    let mut presenter_hooks = TestHooks {
        observer: Some(Arc::clone(&observer)),
        ..TestHooks::default()
    };
    if plan == Plan::EarlyCloseRetained {
        presenter_hooks.stop_budget = Some(Duration::from_millis(300));
    }
    let checker_hooks = match plan {
        Plan::CheckerFailure => Some(TextureTestHooks {
            error_completion: true,
            ..TextureTestHooks::default()
        }),
        _ => checker_gate.as_ref().map(|gate| TextureTestHooks {
            completion_gate: Some(Arc::clone(gate)),
            ..TextureTestHooks::default()
        }),
    };
    let wake = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    // SAFETY: `ck_window::run` below keeps the window (and so its layer) until the presenter's end is proven, which is the
    // contract of `MetalBackend::with_resources`; the backend is used as nothing but that presenter.
    let backend = unsafe {
        MetalBackend::with_resources_and_hooks(
            wake,
            service.clone(),
            relay.clone(),
            Arc::clone(&context),
            limits(),
            BackendTestHooks {
                presenter: presenter_hooks,
                checker: checker_hooks,
                textures: None,
                before_exchange: None,
                ordinary_baseline: true,
            },
        )
    };
    let renderer = Renderer::new(
        ResSpy {
            inner: backend,
            probe: Arc::clone(&probe),
            observer: Arc::clone(&observer),
        },
        Arc::clone(&bridge),
    );
    let pool_slot = Arc::new(Mutex::new(None));
    let ui = ResScript {
        probe: Arc::clone(&probe),
        observer: Arc::clone(&observer),
        plan,
        service: service.clone(),
        context: Arc::clone(&context),
        scene,
        requests: writer,
        pool_slot: Arc::clone(&pool_slot),
        checker_gate: checker_gate.clone(),
        textures,
        quads,
        stage: ResStage::First,
        opened: false,
        passes: 0,
        baseline: None,
        moved_frames: 0,
        release_frames: 0,
        reuploaded: false,
        close_delay: if plan == Plan::MovementReupload {
            200
        } else {
            0
        },
        answers: Vec::new(),
    };
    // The early-close scenario releases the checker's upload only after the backend was asked to stop.
    let releaser = (plan == Plan::EarlyClose).then(|| {
        let (probe, gate) = (Arc::clone(&probe), checker_gate.clone().expect("a gate"));
        thread::spawn(move || {
            let limit = Instant::now() + Duration::from_secs(30);
            while !probe.stop_requested.load(Ordering::SeqCst) && Instant::now() < limit {
                thread::yield_now();
            }
            gate.store(1, Ordering::SeqCst);
        })
    });
    let cocoa = match CocoaBackend::new() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            return ExitCode::FAILURE;
        }
    };
    guard_the_keyboard();
    let (running, ticker) = start_ticker(&notifier);
    let quit = AtomicBool::new(false);
    let end = run(
        cocoa,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = ticker.join();
    if let Some(releaser) = releaser {
        let _ = releaser.join();
    }
    let mut problems = resource_run_problems(plan, &end, &probe, &observer, &bridge, &relay);
    // The end: every CPU holder is gone before any resource count is claimed zero. The scene writer and the backend's slot
    // went with the loop (`run` consumed them); the bridge is detached, so its last published snapshot is gone; what is
    // left is the worker's own handles.
    drop(bridge);
    let pool = pool_slot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(mut pool) = pool {
        let shutdown = pool.shutdown(Duration::from_secs(10));
        if !shutdown.unfinished.is_empty() {
            problems.push(format!("loader threads did not finish: {shutdown:?}"));
        }
    }
    if matches!(plan, Plan::Lifecycle | Plan::MovementReupload) {
        let _ = service.finish_teardown();
        let stats = service.stats();
        if stats.jobs.reserved != 0
            || stats.cpu_pixels.reserved != 0
            || stats.metadata.reserved != 0
        {
            problems.push(format!("a budget is not zero after the end: {stats:?}"));
        }
        if stats.registry.live != 0 || stats.registry.retiring != 0 {
            problems.push(format!(
                "a texture is still live after the end: {:?}",
                stats.registry
            ));
        }
        if stats.loaders.referenced_at_teardown != 0 {
            problems.push(format!(
                "{} resource(s) were still referenced at teardown",
                stats.loaders.referenced_at_teardown
            ));
        }
    }
    if problems.is_empty() {
        println!("worker: {} ended {end:?}", plan.name());
        ExitCode::SUCCESS
    } else {
        for problem in problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}

/// What is wrong with how a resource run went, if anything.
fn resource_run_problems(
    plan: Plan,
    end: &LoopEnd,
    probe: &ResProbe,
    observer: &Observer,
    bridge: &SceneBridge,
    relay: &WakeRelay,
) -> Vec<String> {
    let mut problems: Vec<String> = probe
        .violations
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let failures = probe.failures.load(Ordering::SeqCst);
    match plan {
        Plan::Lifecycle | Plan::MovementReupload | Plan::DelayedChecker | Plan::EarlyClose => {
            if *end != LoopEnd::Closed {
                problems.push(format!(
                    "the loop ended with {end:?}, not Closed (a proven teardown)"
                ));
            }
            if failures != 0 {
                problems.push(format!("{failures} presenter failure(s) were reported"));
            }
        }
        Plan::EarlyCloseRetained => {
            if !matches!(end, LoopEnd::Unproven { .. }) {
                problems.push(format!(
                    "the loop ended with {end:?}, not Unproven (the held upload was retained)"
                ));
            }
        }
        Plan::CheckerFailure => {
            if !matches!(end, LoopEnd::PresenterFailed { .. }) {
                problems.push(format!("the loop ended with {end:?}, not PresenterFailed"));
            }
            if failures != 1 {
                problems.push(format!(
                    "{failures} presenter failure report(s), not exactly one"
                ));
            }
        }
    }
    if matches!(
        plan,
        Plan::CheckerFailure | Plan::EarlyClose | Plan::EarlyCloseRetained
    ) && observer.committed() != 0
    {
        problems.push(format!(
            "{} frame(s) were committed, not none",
            observer.committed()
        ));
    }
    if matches!(plan, Plan::Lifecycle | Plan::MovementReupload) {
        let drawn = probe
            .drawn
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        if drawn < 4 {
            problems.push(format!("only {drawn} distinct generation(s) were drawn"));
        }
    }
    if CLOSE_REQUESTS.load(Ordering::SeqCst) != 0 {
        problems.push(
            "a close request reached the UI: the keyboard filter let Escape through".to_owned(),
        );
    }
    if !matches!(end, LoopEnd::Unproven { .. }) {
        if bridge.phase() != Phase::Ended {
            problems.push(format!("the bridge is {:?}, not Ended", bridge.phase()));
        }
        if !relay_is_disconnected(relay) {
            problems.push("the service's wake relay is still connected after the end".to_owned());
        }
    }
    problems
}

/// Scene stats with the given frame count and last-frame counters (everything else zero).
fn stats_of(frames: u64, instances: u64, resolved: u64, checker: u64) -> SceneStats {
    SceneStats {
        frames,
        last: FrameCounters {
            instances,
            resolved,
            checker,
            ..FrameCounters::default()
        },
        ..SceneStats::default()
    }
}

/// A windowless worker: the two pure checks of the resource scenarios, on synthetic values, with their negative controls.
fn oracles_worker() -> ExitCode {
    // The movement check sees a texture destroyed and created again that occupancy cannot: the same live count, and (as the
    // revisions are not compared here) the same revisions.
    let before = Work {
        textures_created: 5,
        uploads_submitted: 3,
        uploads_completed: 3,
        live_textures: 2,
    };
    assert!(movement_work_problems(before, before).is_empty());
    let recreated = Work {
        textures_created: 6,
        ..before
    };
    assert_eq!(recreated.live_textures, before.live_textures);
    let problems = movement_work_problems(before, recreated);
    assert!(
        problems.len() == 1 && problems[0].contains("textures created"),
        "{problems:?}"
    );
    let reuploaded = Work {
        uploads_submitted: 4,
        uploads_completed: 4,
        ..before
    };
    let problems = movement_work_problems(before, reuploaded);
    assert!(
        problems.iter().any(|p| p.contains("submitted"))
            && problems.iter().any(|p| p.contains("completed")),
        "{problems:?}"
    );
    let fewer_live = Work {
        live_textures: 1,
        ..before
    };
    assert_eq!(movement_work_problems(before, fewer_live).len(), 1);

    // The revisions are compared separately: a new requested or shown revision of either texture is movement.
    let revisions = ([1, 1], [Some(1), Some(1)]);
    assert!(!revisions_moved(revisions, revisions));
    assert!(revisions_moved(revisions, ([2, 1], [Some(1), Some(1)])));
    assert!(revisions_moved(revisions, ([1, 1], [Some(1), Some(2)])));

    // The release step's bound may equal the movement's end (the shown acknowledgement precedes the committing frame), and is
    // wrong only from before it.
    assert!(!release_bound_is_stale(130, 130));
    assert!(!release_bound_is_stale(131, 130));
    assert!(release_bound_is_stale(129, 130));
    assert!(release_bound_is_stale(0, 130));

    // The release check: retirement alone does not complete it; neither does a frame drawn before the release (the movement
    // frames); a frame after it must show the surviving quad alone, resolved.
    let release_frames = 100;
    let surviving = stats_of(101, 1, 1, 0);
    assert!(release_is_complete(1, 0, &surviving, release_frames));
    // Retired, but no frame since the release (the reviewer's schedule: movement frames are far beyond the movement's
    // baseline, and still do not count).
    assert!(!release_is_complete(
        1,
        0,
        &stats_of(100, 1, 1, 0),
        release_frames
    ));
    assert!(!release_is_complete(
        1,
        0,
        &stats_of(100, 2, 2, 0),
        release_frames
    ));
    assert!(!release_is_complete(1, 0, &stats_of(115, 2, 2, 0), 130));
    // A frame after the release that still shows the destroyed quad, or a checker, is not the updated scene.
    assert!(!release_is_complete(
        1,
        0,
        &stats_of(101, 2, 2, 0),
        release_frames
    ));
    assert!(!release_is_complete(
        1,
        0,
        &stats_of(101, 1, 0, 1),
        release_frames
    ));
    // And the frame without retirement is not enough either.
    assert!(!release_is_complete(2, 0, &surviving, release_frames));
    assert!(!release_is_complete(1, 1, &surviving, release_frames));
    println!("worker: the movement and release checks hold on their controls");
    ExitCode::SUCCESS
}

/// A windowless worker: a `with_resources` backend whose start is refused or fails leaves the relay disconnected.
fn relay_worker() -> ExitCode {
    let relay = WakeRelay::default();
    let wakes = ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let service = ResourceService::with_wakes(ResourceLimits::DEFAULT, Mode::Backend, wakes)
        .expect("a backend-mode service");
    let context = MetalContext::new().expect("a Metal device");
    let desc = WindowDesc {
        title: "never opened".to_owned(),
        width: 200,
        height: 150,
        clear: [0.0; 3],
    };
    let pixel_size = PixelSize {
        width: 200,
        height: 150,
    };
    let mut problems = Vec::new();
    for case in [
        "a refused start (the wrong sink)",
        "a presenter start failure (no layer)",
    ] {
        // A target the relay is connected to before the start, standing for "connected on an earlier path".
        let delivered = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&delivered);
        relay.connect(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        // SAFETY: the backend is started with no layer at all, which the presenter refuses before it touches anything.
        let mut backend = unsafe {
            MetalBackend::with_resources(
                Arc::new(|| {}),
                service.clone(),
                relay.clone(),
                Arc::clone(&context),
                limits(),
            )
        };
        let made = backend.new_sink();
        let other: Arc<dyn QuadSink> = sink_over_slot(MetalQuadSlot::new(), Arc::new(|| {}));
        let sink = if case.starts_with("a refused") {
            other
        } else {
            made
        };
        // SAFETY: a null layer is refused by the presenter before any use.
        let started =
            unsafe { backend.start_on_layer(std::ptr::null_mut(), &desc, pixel_size, &sink) };
        match started {
            Ok(()) => problems.push(format!("{case}: the start succeeded")),
            Err(reason) if case.starts_with("a refused") => {
                if !reason.contains("not the one this backend made") {
                    problems.push(format!("{case}: refused for another reason: {reason}"));
                }
            }
            Err(reason) => {
                if !reason.contains("null") {
                    problems.push(format!("{case}: failed for another reason: {reason}"));
                }
            }
        }
        (relay.wake())();
        if delivered.load(Ordering::SeqCst) != 0 {
            problems.push(format!(
                "{case}: a service wake still reached the relay's old target"
            ));
        }
        if !relay_is_disconnected(&relay) {
            problems.push(format!("{case}: the relay is still connected"));
        }
        drop(backend);
    }
    // The normal-end path is the lifecycle scenario's; dropping a backend that was never started disconnects too.
    let delivered = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&delivered);
    relay.connect(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    // SAFETY: never started.
    let idle = unsafe {
        MetalBackend::with_resources(
            Arc::new(|| {}),
            service.clone(),
            relay.clone(),
            Arc::clone(&context),
            limits(),
        )
    };
    drop(idle);
    (relay.wake())();
    if delivered.load(Ordering::SeqCst) != 0 {
        problems.push("dropping the backend left the relay connected".to_owned());
    }
    if problems.is_empty() {
        println!("worker: the relay is disconnected on every path");
        ExitCode::SUCCESS
    } else {
        for problem in problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}

// ---------------------------------------------------------------------------------------------
// The legacy quad's first-frame gate and held replacement on a real window (CK-119)
// ---------------------------------------------------------------------------------------------

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
/// The scenario window's clear colour, [0.1, 0.2, 0.3], as bytes.
const CLEAR: [u8; 4] = [26, 51, 77, 255];
/// The first and the replacement image's rectangles, in points (the window is 200 x 150 points).
const FIRST_AT: (f64, f64) = (10.0, 10.0);
const SECOND_AT: (f64, f64) = (110.0, 70.0);
const THIRD_AT: (f64, f64) = (150.0, 10.0);
const BLUE: [u8; 4] = [0, 0, 255, 255];
const QUAD_SIDE: f64 = 40.0;
/// Loop passes (about 5 ms each) the first frame is held back before its image is let through: long enough for the
/// scripted UI and the main thread to show they keep running, and for the presenter to have held its first attempt.
const HELD_PASSES: usize = 60;
/// Loop passes a stage may take before the script says the run is stuck.
const STAGE_PASSES: usize = 2000;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
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

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
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

/// A solid `SIDE` x `SIDE` RGBA8 PNG (one stored deflate block), for the scenario's assets directory.
fn solid_png(colour: [u8; 4]) -> Vec<u8> {
    let mut raw = Vec::new();
    for _ in 0..SIDE {
        // Each row starts with the filter type 0 (none).
        raw.extend(std::iter::once(0).chain(colour.repeat(SIDE as usize)));
    }
    let length = u16::try_from(raw.len()).expect("a small image");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&SIDE.to_be_bytes());
    header.extend_from_slice(&SIDE.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png_chunk(&mut out, *b"IHDR", &header);
    png_chunk(&mut out, *b"IDAT", &zlib);
    png_chunk(&mut out, *b"IEND", &[]);
    out
}

/// The RGBA pixel of a captured (BGRA) frame at a point of the 200-point-wide window.
fn pixel_at(frame: &CapturedFrame, x: f64, y: f64) -> [u8; 4] {
    let scale = f64::from(frame.width) / 200.0;
    // Truncation is the point: the pixel the point falls in.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "window points inside the frame scaled to pixel indices"
    )]
    let (px, py) = ((x * scale) as usize, (y * scale) as usize);
    let offset = (py * frame.width as usize + px) * 4;
    [
        frame.pixels[offset + 2],
        frame.pixels[offset + 1],
        frame.pixels[offset],
        frame.pixels[offset + 3],
    ]
}

/// The pixel in the middle of the quad whose top left is `at`.
fn middle(frame: &CapturedFrame, at: (f64, f64)) -> [u8; 4] {
    pixel_at(frame, at.0 + QUAD_SIDE / 2.0, at.1 + QUAD_SIDE / 2.0)
}

fn lock_generations(generations: &Mutex<Vec<u64>>) -> std::sync::MutexGuard<'_, Vec<u64>> {
    generations.lock().unwrap_or_else(PoisonError::into_inner)
}

fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
    *gate.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
    gate.1.notify_all();
}

/// The producer's pause between building a frame and exchanging it into the frame slot, and the helper thread that runs the
/// renderer's side of the schedule while the producer is stopped.
#[derive(Default)]
struct ExchangePause {
    /// The next publication stops in the hook.
    armed: AtomicBool,
    /// The producer is stopped in the hook.
    paused: AtomicBool,
    /// The helper let the producer go on.
    released: AtomicBool,
    /// The newest frame generation committed when the helper let the producer go on (the second image's frames).
    generation_at_release: AtomicU64,
}

impl ExchangePause {
    /// The hook: stops the producer here, once, until the helper releases it (bounded, so a lost helper cannot hang it).
    fn hook(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        self.paused.store(true, Ordering::SeqCst);
        let limit = Instant::now() + Duration::from_secs(25);
        while !self.released.load(Ordering::SeqCst) && Instant::now() < limit {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompatStage {
    /// The first image's upload is held: the window is up, no frame may be committed.
    Held,
    /// The upload was let through; waiting for the first frame, which must show the image.
    First,
    /// A replacement with a new image and rectangle was written; its upload is held. Frames keep the old image.
    Pending,
    /// A third image was written and its publication is paused before the slot exchange (the producer's pause) while the
    /// renderer is let draw the second; the exchange happened. Waiting for the frame of the third (still pending): it must
    /// keep drawing the second image, never the first.
    AfterExchange,
    /// The third image's upload was let through; waiting for the frame that shows it.
    Replacement,
    /// A broken replacement was refused; the compatibility quad was cleared; waiting for the frame without it.
    Cleared,
    /// The window was asked to close.
    Closing,
}

/// A fake UI VM that drives the legacy lane: the compatibility quad before the window opens, a replacement, a refused
/// replacement and a clear, each judged from the presenter's off-screen captures.
struct CompatScript {
    probe: Arc<ResProbe>,
    observer: Arc<Observer>,
    lane: LegacyLane,
    scene: SceneWriter,
    requests: RequestWriter,
    store_gate: Arc<(Mutex<bool>, Condvar)>,
    third_gate: Arc<(Mutex<bool>, Condvar)>,
    order: Arc<ExchangePause>,
    /// The generation of every frame committed, in commit order (entry `k` belongs to `captured_frame_at(k)`).
    generations: Arc<Mutex<Vec<u64>>>,
    stage: CompatStage,
    opened: bool,
    /// Loop passes since the window opened, and since the stage began.
    passes: usize,
    stage_passes: usize,
    answers: Vec<UiAnswer>,
}

impl CompatScript {
    fn enter(&mut self, stage: CompatStage) {
        self.stage = stage;
        self.stage_passes = 0;
    }

    /// The latest captured frame, if any.
    fn frame(&self) -> Option<CapturedFrame> {
        self.observer.captured_frame()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the script's stages read best as one ordered match"
    )]
    fn step(&mut self) {
        // The loop posts `Opened` with the first frame, so the script runs from the first pass: while the first frame is
        // held back it is the proof that the main thread and the UI keep running.
        self.passes += 1;
        self.stage_passes += 1;
        let Some(stats) = self.probe.stats() else {
            return;
        };
        if self.stage_passes > STAGE_PASSES && self.stage != CompatStage::Closing {
            self.probe
                .violate(format!("stuck in {:?}: {stats:?}", self.stage));
            self.requests.close();
            self.enter(CompatStage::Closing);
            return;
        }
        match self.stage {
            CompatStage::Held => {
                if self.opened
                    || self.observer.committed() > 0
                    || self.probe.drawn.drawn().is_some()
                {
                    self.probe.violate(
                        "a frame was committed while the legacy quad's image was pending"
                            .to_owned(),
                    );
                }
                if self.passes >= HELD_PASSES && stats.gate_holds >= 1 {
                    release(&self.store_gate);
                    self.enter(CompatStage::First);
                }
            }
            CompatStage::First => {
                let Some(frame) = self.frame().filter(|_| stats.frames >= 1) else {
                    return;
                };
                if (
                    stats.last.instances,
                    stats.last.resolved,
                    stats.last.checker,
                ) != (1, 1, 0)
                {
                    self.probe.violate(format!(
                        "the first frame was not the image alone: {stats:?}"
                    ));
                }
                if middle(&frame, FIRST_AT) != RED {
                    self.probe.violate(format!(
                        "the first frame did not show the image: {:?}",
                        middle(&frame, FIRST_AT)
                    ));
                }
                if stats.gate_longest_hold_micros == 0 {
                    self.probe
                        .violate("the first frame's hold was not measured".to_owned());
                }
                // The replacement: a new image at a new rectangle; its upload is held by the store.
                if let Err(error) = self.lane.quad(
                    &self.scene,
                    "b.png",
                    SECOND_AT.0,
                    SECOND_AT.1,
                    Some(QUAD_SIDE),
                    Some(QUAD_SIDE),
                ) {
                    self.probe
                        .violate(format!("the replacement was refused: {error}"));
                }
                if !self.scene.publish() {
                    self.probe
                        .violate("the replacement was not published".to_owned());
                }
                self.enter(CompatStage::Pending);
            }
            CompatStage::Pending => {
                // Frames keep the old image at the old rectangle and nothing at the new one, until a frame was drawn that held it.
                let Some(frame) = self.frame().filter(|_| stats.held_compat_frames >= 1) else {
                    return;
                };
                if middle(&frame, FIRST_AT) != RED || middle(&frame, SECOND_AT) != CLEAR {
                    return;
                }
                // The third image is written, and its publication stops between building the frame and the exchange. While it is
                // stopped the helper lets the second image's upload through and waits until the renderer has drawn it from the
                // frame it already holds; only then does the exchange happen, with the third image pending.
                self.order.armed.store(true, Ordering::SeqCst);
                if let Err(error) = self.lane.quad(
                    &self.scene,
                    "c.png",
                    THIRD_AT.0,
                    THIRD_AT.1,
                    Some(QUAD_SIDE),
                    Some(QUAD_SIDE),
                ) {
                    self.probe
                        .violate(format!("the third image was refused: {error}"));
                }
                if !self.scene.publish() {
                    self.probe
                        .violate("the third image was not published".to_owned());
                }
                if !self.order.released.load(Ordering::SeqCst) {
                    self.probe
                        .violate("the producer's pause was never released".to_owned());
                }
                self.enter(CompatStage::AfterExchange);
            }
            CompatStage::AfterExchange => {
                // The frame of the third image's generation: the first committed with a generation newer than the second
                // image's frames. Its own capture (bound by index) is inspected, never an older one.
                let released = self.order.generation_at_release.load(Ordering::SeqCst);
                let found = lock_generations(&self.generations)
                    .iter()
                    .position(|generation| *generation > released);
                let Some(index) = found else { return };
                let Some(generation) = lock_generations(&self.generations).get(index).copied()
                else {
                    return;
                };
                if self.observer.captures() <= i64::try_from(index).unwrap_or(i64::MAX) {
                    return;
                }
                let Some(frame) = u32::try_from(index)
                    .ok()
                    .and_then(|index| self.observer.captured_frame_at(index))
                else {
                    return;
                };
                if generation <= released {
                    self.probe.violate(format!(
                        "the frame inspected after the exchange (generation {generation}) is not newer than the second image's ({released})"
                    ));
                }
                eprintln!(
                    "worker: after the exchange: capture {index} is generation {generation} (the second image's frames end at {released}), {} captures completed",
                    self.observer.captures()
                );
                if middle(&frame, FIRST_AT) != CLEAR
                    || middle(&frame, SECOND_AT) != GREEN
                    || middle(&frame, THIRD_AT) != CLEAR
                {
                    self.probe.violate(format!(
                        "after the exchange the first/second/third rectangles were {:?}/{:?}/{:?}, not clear/green/clear",
                        middle(&frame, FIRST_AT),
                        middle(&frame, SECOND_AT),
                        middle(&frame, THIRD_AT)
                    ));
                }
                release(&self.third_gate);
                self.enter(CompatStage::Replacement);
            }
            CompatStage::Replacement => {
                let Some(frame) = self.frame() else { return };
                if middle(&frame, THIRD_AT) != BLUE {
                    return;
                }
                if middle(&frame, FIRST_AT) != CLEAR || middle(&frame, SECOND_AT) != CLEAR {
                    self.probe
                        .violate("an older image was still drawn with the new one".to_owned());
                }
                // A replacement that fails (a broken file, a missing one) changes nothing and publishes nothing.
                for name in ["broken.png", "missing.png"] {
                    if self
                        .lane
                        .quad(&self.scene, name, 0.0, 0.0, None, None)
                        .is_ok()
                    {
                        self.probe
                            .violate(format!("{name} was accepted as the legacy quad"));
                    }
                }
                if self.scene.publish() {
                    self.probe
                        .violate("a refused replacement changed the scene".to_owned());
                }
                // Clear: the next frame has no compatibility quad, and the hold goes.
                self.lane.clear(&self.scene);
                if !self.scene.publish() {
                    self.probe.violate("the clear was not published".to_owned());
                }
                self.enter(CompatStage::Cleared);
            }
            CompatStage::Cleared => {
                let Some(frame) = self.frame() else { return };
                if middle(&frame, SECOND_AT) != CLEAR || stats.held_compat_revision != 0 {
                    return;
                }
                if middle(&frame, FIRST_AT) != CLEAR {
                    self.probe
                        .violate("the cleared quad's image was still drawn".to_owned());
                }
                if typing_was_dropped() {
                    self.requests.close();
                    self.enter(CompatStage::Closing);
                }
            }
            CompatStage::Closing => {}
        }
    }
}

impl UiLink for CompatScript {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => self.opened = true,
            UiEvent::CloseRequested { .. } => {
                CLOSE_REQUESTS.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => panic!("unexpected UI update in test script: {update:?}"),
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        self.step();
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one scenario's setup, run and ordered teardown read best together"
)]
fn compat_worker() -> ExitCode {
    watchdog();
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: "Codekarl render-2d legacy gate test".to_owned(),
            width: 200,
            height: 150,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    // The assets: two solid images and a file that is not an image.
    let root = env::temp_dir().join(format!("ck-render2d-compat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).expect("a unique assets directory");
    std::fs::write(root.join("a.png"), solid_png(RED)).expect("the first image");
    std::fs::write(root.join("b.png"), solid_png(GREEN)).expect("the second image");
    std::fs::write(root.join("c.png"), solid_png(BLUE)).expect("the third image");
    std::fs::write(root.join("broken.png"), b"not a PNG").expect("the broken file");

    let relay = WakeRelay::default();
    let wakes = ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let service = ResourceService::with_assets(
        ResourceLimits::DEFAULT,
        Mode::Backend,
        wakes,
        Some(Assets::new(&root)),
    )
    .expect("a backend-mode service");
    let lane = LegacyLane::new(service.clone(), Some(Assets::new(&root)));
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut scene = bridge.take_writer().expect("the one writer");
    // The compatibility quad before the window exists: the first frame must show it.
    lane.quad(
        &scene,
        "a.png",
        FIRST_AT.0,
        FIRST_AT.1,
        Some(QUAD_SIDE),
        Some(QUAD_SIDE),
    )
    .expect("the first legacy quad");
    assert!(scene.publish());
    (relay.wake())();

    // The store's first upload is held until the script lets it through, and the second texture's creation is held until
    // the script lets the replacement through.
    let store_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let replacement_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let third_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let order = Arc::new(ExchangePause::default());
    let generations = Arc::new(Mutex::new(Vec::new()));
    let context = MetalContext::new().expect("a Metal device");
    let probe = Arc::new(ResProbe::default());
    let observer = Arc::new(Observer::new());
    let wake = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    // SAFETY: `ck_window::run` below keeps the window (and so its layer) until the presenter's end is proven, which is the
    // contract of `MetalBackend::with_resources`; the backend is used as nothing but that presenter.
    let backend = unsafe {
        MetalBackend::with_resources_and_hooks(
            wake,
            service.clone(),
            relay.clone(),
            Arc::clone(&context),
            limits(),
            BackendTestHooks {
                presenter: TestHooks {
                    observer: Some(Arc::clone(&observer)),
                    capture_frames: true,
                    generation_log: Some(Arc::clone(&generations)),
                    ..TestHooks::default()
                },
                checker: None,
                textures: Some(TextureTestHooks {
                    block_worker: Some(Arc::clone(&store_gate)),
                    block_after_texture: Some(Arc::clone(&replacement_gate)),
                    block_after_texture_at: Some(2),
                    block_after_texture_second: Some((3, Arc::clone(&third_gate))),
                    ..TextureTestHooks::default()
                }),
                before_exchange: Some({
                    let order = Arc::clone(&order);
                    Arc::new(move || order.hook())
                }),
                ordinary_baseline: true,
            },
        )
    };
    let renderer = Renderer::new(
        ResSpy {
            inner: backend,
            probe: Arc::clone(&probe),
            observer: Arc::clone(&observer),
        },
        Arc::clone(&bridge),
    );
    let ui = CompatScript {
        probe: Arc::clone(&probe),
        observer: Arc::clone(&observer),
        lane,
        scene,
        requests: writer,
        store_gate: Arc::clone(&store_gate),
        third_gate: Arc::clone(&third_gate),
        order: Arc::clone(&order),
        generations: Arc::clone(&generations),
        stage: CompatStage::Held,
        opened: false,
        passes: 0,
        stage_passes: 0,
        answers: Vec::new(),
    };
    let cocoa = match CocoaBackend::new() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            return ExitCode::FAILURE;
        }
    };
    guard_the_keyboard();
    // The renderer's side of the paused-publication schedule: while the producer is stopped before the exchange, let the
    // second image through and wait until it is on screen, drawn from the frame the slot already holds.
    let helper = {
        let (order, observer, probe) = (
            Arc::clone(&order),
            Arc::clone(&observer),
            Arc::clone(&probe),
        );
        let (replacement_gate, order_gate) = (Arc::clone(&replacement_gate), Arc::clone(&order));
        let generations = Arc::clone(&generations);
        thread::spawn(move || {
            let limit = Instant::now() + Duration::from_secs(40);
            while !order_gate.paused.load(Ordering::SeqCst) {
                if Instant::now() >= limit || order_gate.released.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(1));
            }
            release(&replacement_gate);
            let shown = loop {
                if let Some(frame) = observer.captured_frame()
                    && middle(&frame, SECOND_AT) == GREEN
                {
                    break Some(frame);
                }
                if Instant::now() >= limit {
                    break None;
                }
                thread::sleep(Duration::from_millis(1));
            };
            match shown {
                Some(frame) if middle(&frame, FIRST_AT) == CLEAR => {}
                Some(_) => probe.violate(
                    "the second image was drawn with the first still on screen".to_owned(),
                ),
                None => probe.violate(
                    "the second image was never drawn while the third was paused".to_owned(),
                ),
            }
            let newest = lock_generations(&generations)
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
            order.generation_at_release.store(newest, Ordering::SeqCst);
            order.released.store(true, Ordering::SeqCst);
        })
    };
    let (running, ticker) = start_ticker(&notifier);
    let quit = AtomicBool::new(false);
    let end = run(
        cocoa,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = ticker.join();
    // A run that stopped early must not leave the store's worker held.
    release(&store_gate);
    release(&replacement_gate);
    release(&third_gate);
    order.released.store(true, Ordering::SeqCst);
    let _ = helper.join();
    let _ = std::fs::remove_dir_all(&root);
    let mut problems =
        resource_run_problems(Plan::Lifecycle, &end, &probe, &observer, &bridge, &relay)
            .into_iter()
            // The lifecycle run's own expectation of four distinct generations is not this scenario's.
            .filter(|problem| !problem.contains("distinct generation"))
            .collect::<Vec<_>>();
    if !typing_was_dropped() {
        problems.push("the typist's keys were never dropped".to_owned());
    }
    if problems.is_empty() {
        println!("worker: res-compat ended {end:?}");
        ExitCode::SUCCESS
    } else {
        for problem in problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}
