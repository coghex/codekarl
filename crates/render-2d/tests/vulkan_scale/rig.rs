//! The worker's rig on `glfw-vulkan` (milestone 5.5, issue 6, part C): one real GLFW window on the main thread, the renderer over a
//! `VulkanSinkSide::with_resources` backend with a real instance, surface, device context and `VulkanPresenter`, and a **scene thread**
//! that runs one phase against it (loads, publishes at a fixed cadence, opens the measurement windows).
//!
//! GLFW's Cocoa window needs the process's main thread, so the window loop (`ck_window::run`) owns it. The presenter belongs to the
//! loop's thread (the backend is moved into the renderer), so the scene thread reaches the metrics through a **mailbox** that the
//! backend wrapper serves on the loop thread at its next pass (`Control`): one command at a time, a reply, never an unbounded wait.
//!
//! This is the Metal harness's rig over the Vulkan path (CK-130's `scale/rig.rs`; the duplication is accepted, named in CK-162's
//! follow-up). What differs is stated where it differs: the window, the instance/surface/context creation, the clock (the harness's
//! own, given to the presenter so one window is one interval on one clock), the captured frames (the presenter's capture hook, drained
//! by the scene thread), the per-frame counters (the presenter's build records) and the validation layer (on for the correctness
//! passes, off for the timing runs; clean means only the one tolerated message).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::vk::Handle as _;
use ck_gpu_vulkan::{
    BindingChoice, BindingNotice, BindlessLimits, CleanupResult as GpuCleanup, DrawnRecord,
    FrameCapture, FrameCaptures, FrameCounters, FrameStats, Instance, InstanceOptions,
    LedgerLimits, Loader, MetricsClock, MetricsReading, MetricsWindow,
    PresenterReport as GpuReport, SceneLimits, SceneSetup, SceneStats, TestHooks, TextureLimits,
    ValidationLogHandle, VulkanContext, VulkanPresenter, create_instance_with,
    destroy_surface_and_instance, load_loader, vk,
};
use ck_render_2d::{
    Filter, Gpu2d, QuadSink, Renderer, SceneBridge, SceneWriter, VulkanResourceLimits,
    VulkanSinkSide, WakeRelay,
};
use ck_resources::{Assets, LoaderPool, Mode, ResourceLimits, ResourceService, ServiceWakes};
use ck_window::{
    Answer, CleanupResult, LoopEnd, LoopSettings, Notifier, PixelSize, PostError, PresenterReport,
    RequestWriter, Requests, UiAnswer, UiEvent, UiLink, WindowDesc, run,
};
use ck_window_glfw::{GlfwBackend, GlfwHandles, VulkanLoaderForGlfw};

use crate::evidence;
use crate::json::J;

unsafe extern "C" {
    fn ck_glfw_test_install_input_filter();
}

/// The cadence of the static phases and of every phase that publishes on a schedule: 60 Hz.
pub(crate) const PERIOD_NS: u64 = 16_666_667;
/// How long a mailbox call may take beyond what it asks for.
const CALL_SLACK: Duration = Duration::from_secs(15);

/// The harness's clock: the texture ledger's own (`ck_gpu_vulkan::ledger_clock_ns`: nanoseconds since the process's first look at it,
/// plus one, never zero). It is the one clock of the run: the ledger stamps its adoptions on it, the presenter takes its marks from it,
/// the windows are placed on it, and the cadence waits on it.
#[must_use]
pub(crate) fn now_ns() -> u64 {
    ck_gpu_vulkan::ledger_clock_ns()
}

// ---------------------------------------------------------------------------------------------
// The mailbox to the presenter
// ---------------------------------------------------------------------------------------------

/// What the scene thread asks of the presenter (served on the loop thread).
pub(crate) enum Command {
    Request(MetricsWindow),
    Adopted(u32),
    End(u32, u64),
    Read(u32, Duration),
    Stats,
}

/// The answer to one [`Command`].
pub(crate) enum Reply {
    Refused(String),
    Done,
    Adopted(Option<u64>),
    Reading(Option<Box<MetricsReading>>),
    Stats(Option<Box<SceneStats>>),
}

#[derive(Default)]
struct Slot {
    command: Option<Command>,
    reply: Option<Reply>,
    /// The loop ended: nothing will be served any more.
    ended: bool,
}

/// The mailbox: one command in flight.
#[derive(Default)]
pub(crate) struct Control {
    slot: Mutex<Slot>,
    changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Control {
    /// Posts `command`, wakes the loop and waits (at most `within` plus a slack) for the answer; `None` when the loop ended or did
    /// not answer.
    pub(crate) fn call(
        &self,
        notifier: &Notifier,
        command: Command,
        within: Duration,
    ) -> Option<Reply> {
        let limit = Instant::now() + within + CALL_SLACK;
        let mut slot = lock(&self.slot);
        while (slot.command.is_some() || slot.reply.is_some()) && !slot.ended {
            let now = Instant::now();
            if now >= limit {
                return None;
            }
            slot = self
                .changed
                .wait_timeout(slot, limit - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if slot.ended {
            return None;
        }
        slot.command = Some(command);
        drop(slot);
        notifier.wake();
        let mut slot = lock(&self.slot);
        loop {
            if let Some(reply) = slot.reply.take() {
                self.changed.notify_all();
                return Some(reply);
            }
            let now = Instant::now();
            if slot.ended || now >= limit {
                slot.command = None;
                return None;
            }
            slot = self
                .changed
                .wait_timeout(slot, limit - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Answers the pending command, if any (the loop thread, which owns the presenter).
    fn serve(&self, presenter: Option<&VulkanPresenter>) {
        let Some(command) = lock(&self.slot).command.take() else {
            return;
        };
        let started = "the presenter is not started".to_owned();
        let reply = match (command, presenter) {
            (Command::Request(window), Some(presenter)) => {
                match presenter.request_metrics(window) {
                    Ok(()) => Reply::Done,
                    Err(error) => Reply::Refused(format!("{error:?}")),
                }
            }
            (Command::Adopted(id), Some(presenter)) => {
                Reply::Adopted(presenter.metrics_adopted(id))
            }
            (Command::End(id, end), Some(presenter)) => match presenter.end_metrics(id, end) {
                Ok(()) => Reply::Done,
                Err(error) => Reply::Refused(format!("{error:?}")),
            },
            (Command::Read(id, within), Some(presenter)) => {
                Reply::Reading(presenter.read_metrics(id, within).map(Box::new))
            }
            (Command::Stats, Some(presenter)) => {
                Reply::Stats(Some(Box::new(presenter.scene_stats())))
            }
            (Command::Adopted(_), None) => Reply::Adopted(None),
            (Command::Read(..), None) => Reply::Reading(None),
            (Command::Stats, None) => Reply::Stats(None),
            (Command::Request(_) | Command::End(..), None) => Reply::Refused(started),
        };
        lock(&self.slot).reply = Some(reply);
        self.changed.notify_all();
    }

    /// The loop ended: every waiter gives up.
    fn end(&self) {
        lock(&self.slot).ended = true;
        self.changed.notify_all();
    }
}

// ---------------------------------------------------------------------------------------------
// What the loop thread saw, and the captured frames
// ---------------------------------------------------------------------------------------------

/// The captured frames, drained: the presenter's capture hook appends every drawn frame (8 MiB each at 1920 x 1080) to a vector; a
/// drain thread takes them out every few milliseconds, keeping only their number and the newest, so a run does not hold them all. A
/// frame's index is its place in commit order, as the Metal observer's capture count gave it: the newest of `n` captures is frame
/// `n - 1`.
#[derive(Default)]
pub(crate) struct Captures {
    sink: FrameCaptures,
    taken: Mutex<Taken>,
}

/// How many recent captures are kept to be found by their submission number (8 MiB each; correctness jobs only).
const KEPT_CAPTURES: usize = 8;

#[derive(Default)]
struct Taken {
    /// Captures so far.
    count: u64,
    /// The newest captures, oldest first.
    recent: VecDeque<Arc<FrameCapture>>,
}

impl Captures {
    /// The hook the presenter's capture writes into.
    pub(crate) fn sink(&self) -> FrameCaptures {
        Arc::clone(&self.sink)
    }

    /// Takes what the presenter has captured since the last call: the number of captures so far, and the newest.
    pub(crate) fn poll(&self) -> (u64, Option<Arc<FrameCapture>>) {
        let mut taken = lock(&self.taken);
        self.take_into(&mut taken);
        (taken.count, taken.recent.back().cloned())
    }

    /// The capture of the frame submitted as `submission`, if it is among the recent ones; else the newest submission number seen
    /// (so a caller can tell a capture not yet taken from one already pushed out).
    pub(crate) fn find(&self, submission: u64) -> (Option<Arc<FrameCapture>>, Option<u64>) {
        let mut taken = lock(&self.taken);
        self.take_into(&mut taken);
        let found = taken
            .recent
            .iter()
            .find(|frame| frame.submission == submission)
            .cloned();
        (found, taken.recent.back().map(|frame| frame.submission))
    }

    /// The sequence number the next capture will have (the first capture ever is 0): a cursor taken now bounds a later [`since`] at
    /// its lower end, so that earlier captures are not part of it.
    ///
    /// [`since`]: Self::since
    pub(crate) fn cursor(&self) -> u64 {
        let mut taken = lock(&self.taken);
        self.take_into(&mut taken);
        taken.count
    }

    /// The captures with sequence number `from` or higher that are still kept, oldest first, with their numbers, and **how many of
    /// those numbers are gone** (the last [`KEPT_CAPTURES`] are kept, so a caller that waits for more than that many captures to arrive
    /// between calls loses the older ones, and is told how many instead of not noticing). The `n`th kept capture has the number
    /// `count - kept + n`: the queue is in sequence order and the newest has the highest number.
    pub(crate) fn since(&self, from: u64) -> (u64, Vec<(u64, Arc<FrameCapture>)>) {
        let mut taken = lock(&self.taken);
        self.take_into(&mut taken);
        let first_kept = taken.count - taken.recent.len() as u64;
        let lost = first_kept.saturating_sub(from);
        let frames = taken
            .recent
            .iter()
            .zip(first_kept..)
            .filter(|(_, number)| *number >= from)
            .map(|(frame, number)| (number, Arc::clone(frame)))
            .collect();
        (lost, frames)
    }

    fn take_into(&self, taken: &mut Taken) {
        let mut fresh = lock(&self.sink);
        taken.count += fresh.len() as u64;
        let keep_from = fresh.len().saturating_sub(KEPT_CAPTURES);
        for frame in fresh.drain(keep_from..) {
            taken.recent.push_back(Arc::new(frame));
        }
        fresh.clear();
        while taken.recent.len() > KEPT_CAPTURES {
            taken.recent.pop_front();
        }
    }
}

/// The ordinal of the frame with `submission` among the presenter's queued-frame records, if it is there.
pub(crate) fn ordinal_of(records: &VecDeque<DrawnRecord>, submission: u64) -> Option<u64> {
    records
        .iter()
        .find(|record| record.submission == submission)
        .map(|record| record.ordinal)
}

/// What the loop thread saw, for the scene thread and the final record.
#[derive(Default)]
pub(crate) struct Seen {
    /// The drawable size in pixels, packed `width << 32 | height` (0: none yet).
    pub(crate) pixels: AtomicU64,
    /// The first frame was queued.
    pub(crate) first_frame: AtomicBool,
    /// A presenter failure, once.
    pub(crate) failure: Mutex<Option<String>>,
    /// The presenter's last numbers, taken when its end was observed.
    pub(crate) last_stats: Mutex<Option<SceneStats>>,
    /// The whole run's frame figures, taken at the same moment.
    pub(crate) last_totals: Mutex<Option<FrameStats>>,
    /// The device context, held for the allocation count (`vkAllocateMemory` against `maxMemoryAllocationCount`) and **dropped before
    /// the instance is destroyed**: the destroy helper refuses while anything holds the instance.
    context: Mutex<Option<Arc<VulkanContext>>>,
    /// The device facts for the machine record (taken at the presenter's start).
    pub(crate) device: Mutex<Option<J>>,
    /// Validation messages that were not tolerated (any is a problem), and the tolerated ones.
    pub(crate) validation: Mutex<Vec<String>>,
    pub(crate) tolerated: AtomicUsize,
    /// Whether the worker asked for the layer in its instance options. What the loader actually inserted is read by the supervisor from
    /// the loader's own log (`validation_layer_loaded_per_loader_log`).
    pub(crate) validation_on: AtomicBool,
    /// The ledger's once-per-run binding notice (the bindless table's, or the fallback's), taken from the presenter when it first has it.
    pub(crate) notice: Mutex<Option<BindingNotice>>,
}

impl Seen {
    /// The drawable size, once known.
    pub(crate) fn pixel_size(&self) -> Option<(u32, u32)> {
        match self.pixels.load(Ordering::Acquire) {
            0 => None,
            packed => Some((
                u32::try_from(packed >> 32).unwrap_or(0),
                u32::try_from(packed & 0xFFFF_FFFF).unwrap_or(0),
            )),
        }
    }

    fn set_pixels(&self, width: u32, height: u32) {
        self.pixels.store(
            (u64::from(width) << 32) | u64::from(height),
            Ordering::Release,
        );
    }

    /// The live `vkAllocateMemory` count and the device's limit, while the context is held.
    pub(crate) fn allocations(&self) -> Option<(u32, u32)> {
        lock(&self.context)
            .as_ref()
            .map(|context| context.allocation_count())
    }
}

// ---------------------------------------------------------------------------------------------
// The backend wrapper
// ---------------------------------------------------------------------------------------------

/// One worker's configuration.
pub(crate) struct Config {
    pub(crate) title: &'static str,
    /// The window's size in points.
    pub(crate) points: (u32, u32),
    pub(crate) texture: TextureLimits,
    pub(crate) ledger: LedgerLimits,
    pub(crate) max_textures: usize,
    pub(crate) bindless_slots: u32,
    pub(crate) max_quads: usize,
    pub(crate) binding_geometry_bytes: u64,
    pub(crate) service: ResourceLimits,
    /// The per-revision mode (CK-157's per-texture batches), the baseline S-bindless is measured against; `false`: the bindless table.
    pub(crate) ordinary: bool,
    /// Every frame is also copied off the swap chain and kept (the verification views).
    pub(crate) capture: bool,
    /// The Khronos validation layer (with synchronization validation): on for the correctness passes, off for the timing runs.
    pub(crate) validation: bool,
    pub(crate) assets: Option<PathBuf>,
    pub(crate) scene_max_quads: u32,
    /// The whole-process deadline.
    pub(crate) deadline: Duration,
}

/// The instance, surface and presenter of a started run.
struct Started {
    instance: Option<Instance>,
    surface: vk::SurfaceKHR,
    log: ValidationLogHandle,
    presenter: Option<VulkanPresenter>,
}

/// The Vulkan backend of the rig: `VulkanSinkSide::with_resources`, a real instance, surface and context, and the scene presenter.
/// It serves the mailbox on the loop thread and keeps what the loop saw. Everything else is a forward.
struct Wrapped {
    loader: Loader,
    side: VulkanSinkSide,
    control: Arc<Control>,
    seen: Arc<Seen>,
    captures: Arc<Captures>,
    drawn: Arc<Mutex<Vec<DrawnRecord>>>,
    config: BackendConfig,
    wake: Arc<dyn Fn() + Send + Sync>,
    clock: MetricsClock,
    started: Option<Started>,
}

/// The parts of [`Config`] the backend needs.
#[derive(Clone)]
struct BackendConfig {
    texture: TextureLimits,
    ledger: LedgerLimits,
    max_textures: usize,
    bindless_slots: u32,
    max_quads: usize,
    binding_geometry_bytes: u64,
    ordinary: bool,
    capture: bool,
    validation: bool,
}

impl Wrapped {
    fn presenter(&self) -> Option<&VulkanPresenter> {
        self.started.as_ref().and_then(|s| s.presenter.as_ref())
    }

    /// Notes validation messages that are not tolerated (any is a problem).
    fn check_validation(&self, when: &str) {
        let Some(started) = &self.started else { return };
        let messages = started.log.messages();
        if !messages.is_empty() {
            lock(&self.seen.validation).push(format!("{when}: {messages:?}"));
        }
        self.seen
            .tolerated
            .store(started.log.tolerated().len(), Ordering::SeqCst);
    }
}

impl Gpu2d for Wrapped {
    type Handles = GlfwHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.side.new_sink()
    }

    fn start(
        &mut self,
        handles: GlfwHandles,
        _desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.seen.set_pixels(pixel_size.width, pixel_size.height);
        let frames = self.side.accept_frames(&sink)?;
        let driver = self.side.resident_driver()?;
        let instance = create_instance_with(
            &self.loader,
            handles.required_instance_extensions(),
            InstanceOptions {
                validation: self.config.validation,
                synchronization: self.config.validation,
            },
        )
        .map_err(|error| error.to_string())?;
        self.seen
            .validation_on
            .store(self.config.validation, Ordering::SeqCst);
        let log = instance.validation_log();
        let raw = usize::try_from(instance.raw_handle())
            .map_err(|_| "the instance handle does not fit a pointer".to_owned())?;
        // SAFETY: `raw` is the live `VkInstance` of `instance`, which this function holds for the call; this runs on the main
        // thread (the loop's thread) and the window behind `handles` is alive (the loop destroys it only after the surface).
        let surface =
            unsafe { handles.create_surface(std::ptr::without_provenance_mut::<c_void>(raw)) }
                .map(vk::SurfaceKHR::from_raw)?;
        let context =
            VulkanContext::new(&instance, Some(surface)).map_err(|error| error.to_string())?;
        *lock(&self.seen.context) = Some(Arc::clone(&context));
        let caps = context.bindless_caps();
        *lock(&self.seen.device) = Some(J::o([
            ("device", J::s(caps.device.clone())),
            ("driver", J::s(caps.driver.clone())),
            ("timing", J::s(format!("{:?}", context.timing_caps()))),
        ]));
        let mut hooks = TestHooks::default();
        hooks.scene.drawn = Some(Arc::clone(&self.drawn));
        if self.config.capture {
            hooks.capture_frames = Some(self.captures.sink());
        }
        let binding = if self.config.ordinary {
            BindingChoice::PerRevision
        } else {
            BindingChoice::Auto(BindlessLimits {
                texture_slots: self.config.bindless_slots,
            })
        };
        let setup = SceneSetup::with_binding_choice(
            context,
            self.config.texture,
            self.config.ledger,
            self.config.max_textures,
            binding,
            SceneLimits {
                max_quads: self.config.max_quads,
                binding_geometry_bytes: self.config.binding_geometry_bytes,
            },
            driver,
            frames,
            None,
        )
        .with_clock(Arc::clone(&self.clock));
        let wake = Arc::clone(&self.wake);
        // SAFETY: `surface` was created on `instance` for the window `ck_window::run` holds, which outlives the presenter's proven
        // end (the loop destroys the window only after a `Proven` end, and never otherwise); the context was made for this surface;
        // nothing else uses the surface meanwhile, and the surface and the instance are destroyed only after that end, in that order
        // (`poll_end`).
        let presenter = unsafe {
            VulkanPresenter::start_scene_with_test_hooks(
                surface,
                (pixel_size.width, pixel_size.height),
                [0.1, 0.2, 0.3],
                wake,
                &setup,
                hooks,
            )
        };
        // The setup's holds go now: the presenter's thread keeps its own.
        drop(setup);
        let presenter = match presenter {
            Ok(presenter) => presenter,
            Err(error) => {
                let _ = self.side.disconnect();
                lock(&self.seen.context).take();
                // SAFETY: the surface belongs to `instance`; no presenter was started, so nothing uses it.
                let _ = unsafe { destroy_surface_and_instance(instance, surface, None) };
                return Err(error.to_string());
            }
        };
        if let Err(reason) = self.side.connect(presenter.texture_wake()) {
            lock(&self.seen.failure)
                .get_or_insert(format!("the relay's connect returned Err: {reason}"));
        }
        self.started = Some(Started {
            instance: Some(instance),
            surface,
            log,
            presenter: Some(presenter),
        });
        Ok(())
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        if let Some(presenter) = self.presenter() {
            presenter.set_clear(rgb);
        }
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.seen.set_pixels(width, height);
        if let Some(presenter) = self.presenter() {
            presenter.set_pixel_size(width, height);
        }
    }

    fn request_redraw(&mut self) {
        if let Some(presenter) = self.presenter() {
            presenter.request_redraw();
        }
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.control.serve(self.presenter());
        if let Some(notice) = self
            .presenter()
            .and_then(VulkanPresenter::take_binding_notice)
        {
            *lock(&self.seen.notice) = Some(notice);
        }
        if let Some(report) = self.side.take_report() {
            return Some(report);
        }
        let report = self.presenter().and_then(VulkanPresenter::take_report)?;
        Some(match report {
            GpuReport::FirstFrameQueued => {
                self.seen.first_frame.store(true, Ordering::Release);
                PresenterReport::FirstFrameQueued
            }
            GpuReport::Failed { message } => {
                *lock(&self.seen.failure) = Some(message.clone());
                PresenterReport::Failed { message }
            }
        })
    }

    fn request_stop(&mut self) {
        if let Some(presenter) = self.presenter() {
            presenter.request_stop();
        }
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.control.serve(self.presenter());
        let ended = self
            .started
            .as_mut()?
            .presenter
            .as_mut()?
            .poll_end(within)?;
        // The presenter's last numbers, while its handle is alive.
        let (stats, totals) = {
            let presenter = self.presenter();
            (
                presenter.map(VulkanPresenter::scene_stats),
                presenter.map(VulkanPresenter::metrics_totals),
            )
        };
        *lock(&self.seen.last_stats) = stats;
        *lock(&self.seen.last_totals) = totals;
        self.control.end();
        self.check_validation("at the presenter's end");
        match ended {
            GpuCleanup::Proven => {
                let started = self.started.as_mut()?;
                started.presenter = None;
                let _ = self.side.disconnect();
                // The harness's hold on the context goes before the instance is destroyed (the helper refuses while anything holds it).
                lock(&self.seen.context).take();
                let instance = started.instance.take()?;
                // SAFETY: `surface` was created on `instance`; the presenter ended `Proven` and its handle is gone, so nothing uses
                // it, and the window it belongs to is alive: the loop destroys the window after this call.
                let destroyed = unsafe {
                    destroy_surface_and_instance(
                        instance,
                        started.surface,
                        Some(&GpuCleanup::Proven),
                    )
                };
                if let Err(refused) = destroyed {
                    let reason = format!("the surface and instance were not destroyed: {refused}");
                    std::mem::forget(refused);
                    return Some(CleanupResult::Unproven { reason });
                }
                self.check_validation("after the destruction");
                Some(CleanupResult::Proven)
            }
            GpuCleanup::Unproven { reason } => {
                let _ = self.side.disconnect();
                Some(CleanupResult::Unproven { reason })
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The UI link
// ---------------------------------------------------------------------------------------------

/// The scripted UI: it notes that the window opened and declines every close request (the owner's keys never get this far; the
/// filter drops them before any window sees them).
struct Ui {
    opened: Arc<AtomicBool>,
    close_requests: Arc<AtomicUsize>,
    answers: Vec<UiAnswer>,
}

impl UiLink for Ui {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => self.opened.store(true, Ordering::Release),
            UiEvent::CloseRequested { .. } => {
                self.close_requests.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => panic!("unexpected UI update in a scale worker: {update:?}"),
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------------------------
// The scene thread's handle
// ---------------------------------------------------------------------------------------------

/// The presenter's numbers a phase reads: its counters, and the counters of the newest frame build.
#[derive(Clone, Copy)]
pub(crate) struct Stats {
    pub(crate) scene: SceneStats,
    /// What the newest committed frame's build counted (all zero before the first frame is queued).
    pub(crate) last: FrameCounters,
    /// The newest committed frame, if any: its ordinal among the presenter's queued frames and the scene generation it drew.
    pub(crate) committed: Option<Committed>,
}

/// A frame the presenter queued for presentation (the presenter's own record of it, not a build).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Committed {
    /// 1 for the first frame the presenter queued.
    pub(crate) ordinal: u64,
    /// The generation of the scene the frame drew.
    pub(crate) generation: Option<u64>,
    /// The number of the frame's submission, which a capture of the same frame carries.
    pub(crate) submission: u64,
}

impl std::ops::Deref for Stats {
    type Target = SceneStats;

    fn deref(&self) -> &SceneStats {
        &self.scene
    }
}

/// What a phase works with, on the scene thread.
pub(crate) struct Scene {
    pub(crate) service: ResourceService,
    pub(crate) writer: SceneWriter,
    pub(crate) requests: RequestWriter,
    pub(crate) control: Arc<Control>,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) captures: Arc<Captures>,
    pub(crate) drawn: Arc<Mutex<VecDeque<DrawnRecord>>>,
    pub(crate) seen: Arc<Seen>,
    pub(crate) opened: Arc<AtomicBool>,
}

/// A fixed-cadence schedule on the harness clock: never faster than the period, a late tick taken once and not caught up.
pub(crate) struct Cadence {
    next: u64,
    /// Ticks taken.
    pub(crate) ticks: u64,
    /// Ticks that were late by more than half a period.
    pub(crate) late: u64,
    /// The latest a tick has been, in nanoseconds.
    pub(crate) worst_late_ns: u64,
}

impl Cadence {
    /// A schedule whose first tick is due at `first`.
    pub(crate) fn starting_at(first: u64) -> Self {
        Self {
            next: first,
            ticks: 0,
            late: 0,
            worst_late_ns: 0,
        }
    }

    /// Waits for the next deadline and returns the time it was taken at.
    pub(crate) fn tick(&mut self) -> u64 {
        let due = self.next;
        let now = now_ns();
        if due > now {
            thread::sleep(Duration::from_nanos(due - now));
        }
        let taken = now_ns();
        let late = taken.saturating_sub(due);
        self.ticks += 1;
        self.worst_late_ns = self.worst_late_ns.max(late);
        if late > PERIOD_NS / 2 {
            self.late += 1;
        }
        // The next deadline is the next whole period after the one just taken: a tick that ran late is not made up.
        let periods = late / PERIOD_NS + 1;
        self.next = due + periods * PERIOD_NS;
        taken
    }
}

impl Scene {
    /// Waits for the window to open and report its drawable size, within `bound`.
    pub(crate) fn wait_open(&self, bound: Duration) -> Result<(u32, u32), String> {
        let limit = Instant::now() + bound;
        loop {
            if let Some(message) = lock(&self.seen.failure).clone() {
                return Err(format!("the presenter failed: {message}"));
            }
            if self.opened.load(Ordering::Acquire)
                && self.seen.first_frame.load(Ordering::Acquire)
                && let Some(size) = self.seen.pixel_size()
            {
                return Ok(size);
            }
            if Instant::now() >= limit {
                return Err("the window did not open in time".to_owned());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// The presenter failed (once), if it did.
    pub(crate) fn failure(&self) -> Option<String> {
        lock(&self.seen.failure).clone()
    }

    fn call(&self, command: Command, within: Duration) -> Option<Reply> {
        self.control.call(&self.notifier, command, within)
    }

    /// Files `window` with the presenter.
    pub(crate) fn request_window(&self, window: MetricsWindow) -> Result<(), String> {
        match self.call(Command::Request(window), Duration::ZERO) {
            Some(Reply::Done) => Ok(()),
            Some(Reply::Refused(why)) => Err(why),
            _ => Err("the presenter did not answer the window request".to_owned()),
        }
    }

    /// Sets the end of window `id`.
    pub(crate) fn end_window(&self, id: u32, end: u64) -> Result<(), String> {
        match self.call(Command::End(id, end), Duration::ZERO) {
            Some(Reply::Done) => Ok(()),
            Some(Reply::Refused(why)) => Err(why),
            _ => Err("the presenter did not answer the window end".to_owned()),
        }
    }

    /// When the presenter adopted window `id`.
    pub(crate) fn adopted(&self, id: u32) -> Option<u64> {
        match self.call(Command::Adopted(id), Duration::ZERO) {
            Some(Reply::Adopted(at)) => at,
            _ => None,
        }
    }

    /// Window `id` once its work is consumed, waiting at most `within`.
    pub(crate) fn read_window(&self, id: u32, within: Duration) -> Option<MetricsReading> {
        match self.call(Command::Read(id, within), within) {
            Some(Reply::Reading(reading)) => reading.map(|reading| *reading),
            _ => None,
        }
    }

    /// The presenter's counters, and the counters of the newest frame the presenter queued (a committed frame, by its own record).
    pub(crate) fn stats(&self) -> Option<Stats> {
        let scene = match self.call(Command::Stats, Duration::ZERO) {
            Some(Reply::Stats(stats)) => stats.map(|stats| *stats)?,
            _ => return None,
        };
        let newest = lock(&self.drawn).back().copied();
        Some(Stats {
            scene,
            last: newest.map(|record| record.counters).unwrap_or_default(),
            committed: newest.map(|record| Committed {
                ordinal: record.ordinal,
                generation: record.generation,
                submission: record.submission,
            }),
        })
    }

    /// The ordinal (1 for the first queued frame) of the committed frame that has this submission number, among the recent ones.
    pub(crate) fn committed_ordinal_of(&self, submission: u64) -> Option<u64> {
        ordinal_of(&lock(&self.drawn), submission)
    }

    /// The cursor to take before an action whose captures are to be read with [`Scene::captures_since`].
    pub(crate) fn captures_cursor(&self) -> u64 {
        self.captures.cursor()
    }

    /// The kept captures from sequence number `from` on, and how many numbers from there are gone.
    pub(crate) fn captures_since(&self, from: u64) -> (u64, Vec<(u64, Arc<FrameCapture>)>) {
        self.captures.since(from)
    }

    /// The capture of the frame submitted as `submission` (among the recent ones), and the newest submission number captured.
    pub(crate) fn capture_with_submission(
        &self,
        submission: u64,
    ) -> (Option<Arc<FrameCapture>>, Option<u64>) {
        self.captures.find(submission)
    }

    /// The live `vkAllocateMemory` count and the device's `maxMemoryAllocationCount`.
    pub(crate) fn allocations(&self) -> Option<(u32, u32)> {
        self.seen.allocations()
    }

    /// The ledger's binding notice, once the presenter has reported it.
    pub(crate) fn binding_notice(&self) -> Option<BindingNotice> {
        lock(&self.seen.notice).clone()
    }

    /// Ends the run: the window closes and the loop's teardown proves the presenter's end.
    pub(crate) fn close(&self) {
        self.requests.close();
        self.notifier.wake();
    }
}

// ---------------------------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------------------------

/// The facts of the end of a worker, for the final record.
pub(crate) struct Ended {
    pub(crate) loop_end: LoopEnd,
    pub(crate) problems: Vec<String>,
    pub(crate) stats: Option<SceneStats>,
    pub(crate) totals: Option<FrameStats>,
    pub(crate) resources: J,
    pub(crate) device: Option<J>,
}

/// A hang ends the process, so the supervisor reports it.
fn watchdog(deadline: Duration) {
    thread::spawn(move || {
        thread::sleep(deadline.saturating_sub(Duration::from_secs(10)));
        eprintln!("worker: the window loop did not finish in time");
        std::process::exit(2);
    });
}

/// A stimulus that wakes the loop: frames produce no report, so an idle loop would otherwise sleep.
fn ticker(notifier: &Arc<Notifier>) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let running = Arc::new(AtomicBool::new(true));
    let handle = {
        let (notifier, running) = (Arc::clone(notifier), Arc::clone(&running));
        thread::spawn(move || {
            while running.load(Ordering::Acquire) {
                notifier.wake();
                thread::sleep(Duration::from_millis(16));
            }
        })
    };
    (running, handle)
}

/// Runs one phase: opens the window, runs `phase` on the scene thread, closes, tears down in the application's order and returns what
/// the phase produced with the end's own checks.
///
/// # Panics
///
/// When the machine has no Vulkan loader or no desktop session (reported, not hidden).
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn run_phase<F>(config: Config, phase: F) -> (Result<J, String>, Ended)
where
    F: FnOnce(&mut Scene) -> Result<J, String> + Send + 'static,
{
    // The owner is using the machine: no window of this run asks the system to activate the application or make a window key.
    ck_window::test_support::open_windows_without_activation();
    crate::activation_watch::install(false);
    watchdog(config.deadline);
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: config.title.to_owned(),
            width: config.points.0,
            height: config.points.1,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    let relay = WakeRelay::default();
    let wakes = ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let service = ResourceService::with_assets(
        config.service,
        Mode::Backend,
        wakes,
        config.assets.map(Assets::new),
    )
    .expect("a backend-mode service");
    let bridge = SceneBridge::with_limits(
        Filter::Nearest,
        ck_render_2d::SceneLimits {
            max_quads: config.scene_max_quads,
        },
    )
    .expect("valid scene limits");
    let scene_writer = bridge.take_writer().expect("the one writer");
    let pool = LoaderPool::start(service.clone()).expect("the loader pool starts");

    let loader = match load_loader() {
        Ok(loader) => loader,
        Err(error) => {
            eprintln!("worker: no Vulkan loader: {error}");
            std::process::exit(1);
        }
    };
    let wake = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    let clock: MetricsClock = Arc::new(now_ns);
    let control = Arc::new(Control::default());
    let seen = Arc::new(Seen::default());
    let captures = Arc::new(Captures::default());
    let drawn_sink: Arc<Mutex<Vec<DrawnRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let backend = Wrapped {
        loader: loader.clone(),
        side: VulkanSinkSide::with_resources(
            Arc::clone(&wake),
            service.clone(),
            relay.clone(),
            VulkanResourceLimits {
                max_adoptions_per_frame: 256,
            },
        ),
        control: Arc::clone(&control),
        seen: Arc::clone(&seen),
        captures: Arc::clone(&captures),
        drawn: Arc::clone(&drawn_sink),
        config: BackendConfig {
            texture: config.texture,
            ledger: config.ledger,
            max_textures: config.max_textures,
            bindless_slots: config.bindless_slots,
            max_quads: config.max_quads,
            binding_geometry_bytes: config.binding_geometry_bytes,
            ordinary: config.ordinary,
            capture: config.capture,
            validation: config.validation,
        },
        wake,
        clock,
        started: None,
    };
    let renderer = Renderer::new(backend, Arc::clone(&bridge));
    let opened = Arc::new(AtomicBool::new(false));
    let close_requests = Arc::new(AtomicUsize::new(0));
    let ui = Ui {
        opened: Arc::clone(&opened),
        close_requests: Arc::clone(&close_requests),
        answers: Vec::new(),
    };
    // The frames the presenter queued (committed) are moved into a bounded queue the scene thread reads the newest of: a long run does
    // not hold every frame's record. Only a queued frame has a record, so a build that was abandoned or held back is never counted.
    let queued: Arc<Mutex<VecDeque<DrawnRecord>>> = Arc::new(Mutex::new(VecDeque::new()));
    let mut scene = Scene {
        service: service.clone(),
        writer: scene_writer,
        requests: writer,
        control: Arc::clone(&control),
        notifier: Arc::clone(&notifier),
        captures: Arc::clone(&captures),
        drawn: Arc::clone(&queued),
        seen: Arc::clone(&seen),
        opened,
    };
    let draining = Arc::new(AtomicBool::new(true));
    let drain = {
        let (sink, queue, draining) = (
            Arc::clone(&drawn_sink),
            Arc::clone(&queued),
            Arc::clone(&draining),
        );
        thread::spawn(move || {
            while draining.load(Ordering::Acquire) {
                let fresh: Vec<DrawnRecord> = std::mem::take(&mut *lock(&sink));
                if !fresh.is_empty() {
                    let mut queue = lock(&queue);
                    queue.extend(fresh);
                    while queue.len() > 64 {
                        queue.pop_front();
                    }
                }
                thread::sleep(Duration::from_millis(2));
            }
        })
    };
    let capture_drain = {
        let (captures, draining) = (Arc::clone(&captures), Arc::clone(&draining));
        thread::spawn(move || {
            while draining.load(Ordering::Acquire) {
                let _ = captures.poll();
                thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let scene_thread = thread::Builder::new()
        .name("scale-scene".to_owned())
        .spawn(move || {
            let result = phase(&mut scene);
            // The phase is over, whatever it came to: the window closes and the loop's teardown proves the presenter's end. The
            // scene's quads (and their texture references) go with `scene`, on this thread, before the loop's end is awaited.
            scene.close();
            drop(scene);
            result
        })
        .expect("the scene thread starts");

    let proc_addr = loader.get_instance_proc_addr();
    // SAFETY: both types are the C-ABI `vkGetInstanceProcAddr` signature (a pointer-sized instance handle and a C string in, a
    // function pointer out), so the conversion keeps the callee; it is valid while a clone of `loader` lives.
    let proc_addr = unsafe {
        std::mem::transmute::<ck_gpu_vulkan::GetInstanceProcAddr, ck_window_glfw::GetInstanceProcAddr>(
            proc_addr,
        )
    };
    // SAFETY: `proc_addr` is this loader's own `vkGetInstanceProcAddr`, and the keep-alive is a clone of that loader, which keeps the
    // library loaded for as long as the value lives (the backend holds it until after `glfwTerminate`).
    let for_glfw = unsafe { VulkanLoaderForGlfw::new(proc_addr, Box::new(loader.clone())) };
    let glfw = match GlfwBackend::new(Some(for_glfw)) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            std::process::exit(1);
        }
    };
    // SAFETY: main thread; installs one local event monitor, once, before the first window.
    unsafe { ck_glfw_test_install_input_filter() };
    let (running, wakeups) = ticker(&notifier);
    let quit = AtomicBool::new(false);
    let loop_end = run(
        glfw,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = wakeups.join();
    draining.store(false, Ordering::Release);
    let _ = drain.join();
    let _ = capture_drain.join();
    control.end();
    let result = scene_thread
        .join()
        .unwrap_or_else(|_| Err("the scene thread panicked".to_owned()));

    let mut problems = Vec::new();
    if !matches!(loop_end, LoopEnd::Closed) {
        problems.push(format!("the window loop ended {loop_end:?}, not Closed"));
    }
    if close_requests.load(Ordering::SeqCst) != 0 {
        problems.push(format!(
            "{} close request(s) reached the UI (the keyboard filter should drop them)",
            close_requests.load(Ordering::SeqCst)
        ));
    }
    for message in lock(&seen.validation).iter() {
        problems.push(format!("validation reported {message}"));
    }
    if seen.tolerated.load(Ordering::SeqCst) > 1 {
        problems.push(format!(
            "{} tolerated validation messages (at most one, once per presenter)",
            seen.tolerated.load(Ordering::SeqCst)
        ));
    }
    // The end: every CPU holder is gone before a resource count is claimed zero. The scene writer and the backend's slot went with the
    // loop and the scene thread; the bridge is detached; what is left is this function's own handles.
    drop(bridge);
    let mut pool = pool;
    let shutdown = pool.shutdown(Duration::from_secs(20));
    if !shutdown.unfinished.is_empty() {
        problems.push(format!("loader threads did not finish: {shutdown:?}"));
    }
    drop(pool);
    let _ = service.finish_teardown();
    let stats = service.stats();
    if stats.jobs.reserved != 0 || stats.cpu_pixels.reserved != 0 || stats.metadata.reserved != 0 {
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
    let resources = J::o([
        ("live_at_end", J::U(u64::from(stats.registry.live))),
        ("retiring_at_end", J::U(u64::from(stats.registry.retiring))),
        ("cpu_pixel_bytes_peak", J::U(stats.cpu_pixels.peak)),
        ("loader_files_decoded", J::U(stats.loaders.files_decoded)),
        ("loader_decoded_bytes", J::U(stats.loaders.decoded_bytes)),
        ("service", evidence::service_stats(&stats)),
        (
            "validation",
            J::o([
                (
                    "layer_requested",
                    J::Bool(seen.validation_on.load(Ordering::SeqCst)),
                ),
                (
                    "tolerated_messages",
                    J::U(seen.tolerated.load(Ordering::SeqCst) as u64),
                ),
            ]),
        ),
    ]);
    let ended = Ended {
        loop_end,
        problems,
        stats: lock(&seen.last_stats).take(),
        totals: lock(&seen.last_totals).take(),
        resources,
        device: lock(&seen.device).take(),
    };
    (result, ended)
}

/// Makes the process exit with the right status after printing the worker's result line.
pub(crate) fn finish(result: &Result<J, String>, ended: &Ended) -> ExitCode {
    let mut problems = ended.problems.clone();
    if let Err(error) = result {
        problems.push(format!("the phase failed: {error}"));
    }
    let mut record = match result {
        Ok(json) => json.clone(),
        Err(_) => J::o([]),
    };
    record.push(
        "end",
        J::o([
            ("loop_end", J::s(format!("{:?}", ended.loop_end))),
            ("resources", ended.resources.clone()),
            ("device", ended.device.clone().unwrap_or(J::Null)),
            (
                "last_scene_stats",
                ended
                    .stats
                    .as_ref()
                    .map_or(J::Null, |stats| evidence::scene_stats(stats, None)),
            ),
            (
                "last_window_totals",
                ended.totals.as_ref().map_or(J::Null, evidence::frame_stats),
            ),
            (
                "problems",
                J::A(problems.iter().cloned().map(J::S).collect()),
            ),
        ]),
    );
    println!("SCALE_RESULT {}", record.render());
    let passed = problems.is_empty() && record.get("passed") == Some(&J::Bool(true));
    println!("SCALE_STATUS passed={passed}");
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        for problem in &problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}
