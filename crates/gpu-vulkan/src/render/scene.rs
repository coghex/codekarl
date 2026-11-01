//! The scene presenter's render thread (milestone 5.5, issue 2a): the frame loop of [`VulkanPresenter::start_scene`], the
//! idle rule and the combined proof.
//!
//! It is the legacy thread's design on the shared context, adapted (the legacy `Run` is untouched, so `start` and
//! `start_2d` keep today's behaviour):
//!
//! - every `vkQueueSubmit` and `vkQueuePresentKHR` goes through the context's [`RenderQueue`];
//! - a frame is submitted through [`FrameTicket::submit`](crate::FrameTicket::submit), whose number is the frame's proof.
//!   The present ledger asks the texture ledger's latched `frame_proven(number)` wherever it asked a submit fence;
//! - there is no unconditional sleep. Runnable work is never delayed; the thread waits only when nothing is runnable, for
//!   the earlier of the pending swap-chain creation retry and 1 ms while GPU work is outstanding, and without a bound
//!   when neither exists (see [`idle_deadline`]).
//!
//! **Why one instance buffer is enough.** At most one frame is in flight: a frame is built only when
//! `frame_proven(previous submission)` is true, which is checked before the first instance is written.

use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use ash::vk;
use ash::vk::Handle as _;

use super::{
    ACQUIRE_TIMEOUT_NS, BACKOFF_MAX, BACKOFF_START, CaptureWait, Fatal, Outcome, POOL_FENCES, Prep,
    Prepared, Residency, VulkanScene, failure,
};
use crate::Error;
use crate::context::{RenderQueue, VulkanContext};
use crate::gpu::{FrameExtras, Gpu};
use crate::gpu_quad::format_supported;
use crate::hooks::{BuildRecord, Event, Observer, TestHooks, WaitKind};
use crate::ledger::{Frame, Ledger, PresentCall, PresentSlot, Presented, Proof};
use crate::revisions::{CheckerProgress, LedgerError};
use crate::scene::{
    Boundary, FrameSeam, SceneBindings, SceneLimits, SceneObject, SceneSetup, wakes_of,
};
use crate::scene_draw::{InstanceBuffer, SceneDraw, ScenePipeline};
use crate::shared::{CleanupResult, PresenterReport, SceneStats, Shared, lock};
use crate::slot::QuadSkipped;
use crate::unwind;
use timing::{HandedOff, PRESENTATION_POLL_MS, Timing};

/// The longest the thread waits while GPU work is outstanding: the 1 ms look at fences and completions.
const GPU_POLL: Duration = Duration::from_millis(1);

/// The thread's entry point: runs the scene presenter and publishes its cleanup result. A panic is caught here and gives
/// `Unproven` with every holder kept.
pub(crate) fn scene_main(
    shared: &Arc<Shared>,
    setup: SceneSetup,
    surface: vk::SurfaceKHR,
    hooks: TestHooks,
) {
    if let Some(started) = &hooks.thread_started {
        started.store(true, Ordering::SeqCst);
    }
    // The thread's own hold on the scene (and through it on the context), dropped only after a proven end.
    let setup = ManuallyDrop::new(setup);
    let outcome = catch_unwind(AssertUnwindSafe(|| run(shared, &setup, surface, hooks)));
    let result = match outcome {
        Ok(Outcome::Proven) => CleanupResult::Proven,
        Ok(Outcome::Unproven(reason)) => CleanupResult::Unproven { reason },
        Err(payload) => {
            // Disposed of without unwinding, so the result below is always stored and the wake sent.
            unwind::dispose(payload);
            CleanupResult::Unproven {
                reason: "the render thread panicked; everything it owned was kept".to_owned(),
            }
        }
    };
    // The presenter is over: no frame will be presented or resolved again, so what never resolved expires now, final.
    shared.metrics.recorder().stop();
    shared.metrics.changed.notify_all();
    // The frame slot stops waking it (a publish from now on only stores the value).
    setup.0.clear_wake();
    if matches!(result, CleanupResult::Proven) {
        drop(ManuallyDrop::into_inner(setup));
    }
    *lock(&shared.result) = Some(result);
    shared.wake();
}

mod quad;
mod timing;

/// What a frame step did.
enum Step {
    /// A frame was queued; `again` asks for another (the present was out of date).
    Drawn { again: bool },
    /// Nothing could be drawn now (the facts it needs change): the redraw stays pending.
    Wait,
    /// Held back by the initial gate: the redraw stays pending, parked by the gate, and the next attempt runs a frame
    /// boundary again, where bounded adoption makes progress.
    Gated,
    /// The window's pixel size is zero: nothing is drawn and no swap chain is created.
    Suspended,
}

/// The facts a wanted frame needs before it is attempted (r3: a want that fails one is parked, not retried).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent readiness facts, each read on its own"
)]
pub(crate) struct Gates {
    /// The checker is resident.
    pub(crate) checker: bool,
    /// The compatibility gate is open (always, without a compatibility key).
    pub(crate) compat: bool,
    /// The drawable size is non-zero.
    pub(crate) sized: bool,
    /// The present ledger can supply a frame and no swap-chain rebuild is paused or backing off.
    pub(crate) capacity: bool,
    /// There is no previous submission, or its proof is recorded.
    pub(crate) previous: bool,
}

impl Gates {
    pub(crate) fn open(self) -> bool {
        self.checker && self.compat && self.sized && self.capacity && self.previous
    }
}

/// Whether a wanted frame is runnable: wanted, not failed, and every gate open.
pub(crate) fn frame_runnable(dirty: bool, failed: bool, gates: Gates) -> bool {
    dirty && !failed && gates.open()
}

/// What decides how long an idle thread waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleFacts {
    /// An upload batch, a submitted frame, release work or a present-ledger fence is outstanding.
    pub(crate) gpu_outstanding: bool,
    /// A swap-chain creation retry is pending: the moment it is due.
    pub(crate) retry_at: Option<Instant>,
    /// A present's timing feedback is pending: the thread polls for it at least every 50 ms.
    pub(crate) presentation_pending: bool,
}

/// The idle wait's deadline and what bounds it: the earliest of the pending creation retry and `now` + 1 ms (only while
/// GPU work is outstanding); `None` when neither exists, which is a wait without a bound.
pub(crate) fn idle_deadline(now: Instant, facts: IdleFacts) -> Option<(Instant, WaitKind)> {
    let poll = facts.gpu_outstanding.then(|| now + GPU_POLL);
    let feedback = facts
        .presentation_pending
        .then(|| now + Duration::from_millis(PRESENTATION_POLL_MS));
    // The earliest of the three; on a tie the retry, then the GPU poll, then the feedback poll.
    let mut best: Option<(Instant, WaitKind)> = None;
    for candidate in [
        facts.retry_at.map(|at| (at, WaitKind::RetryDeadline)),
        poll.map(|at| (at, WaitKind::GpuPoll)),
        feedback.map(|at| (at, WaitKind::PresentationPoll)),
    ]
    .into_iter()
    .flatten()
    {
        if best.is_none_or(|(at, _)| candidate.0 < at) {
            best = Some(candidate);
        }
    }
    best
}

/// Where the frame being built stands with its boundary pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameBoundary {
    /// The next attempt runs the boundary pass first.
    Due,
    /// The pass ran; a retry of the step goes straight to the step.
    Passed,
}

/// The device objects and the present state of the thread.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the thread's independent state flags, each read on its own"
)]
struct Core<'a> {
    shared: &'a Shared,
    hooks: TestHooks,
    gpu: Gpu,
    ledger: Ledger,
    queue: ManuallyDrop<RenderQueue>,
    context: ManuallyDrop<Arc<VulkanContext>>,
    buffer: InstanceBuffer,
    pipeline: Option<ScenePipeline>,
    bindings: SceneBindings,
    limits: SceneLimits,
    image_count: u32,
    first_reported: bool,
    failed: bool,
    presented: u64,
    rebuild_wanted: bool,
    suboptimal_pending: bool,
    creation_backoff: Duration,
    creation_retry_at: Option<Instant>,
    creation_failed_epoch: u64,
    observer: Option<Arc<Observer>>,
    /// The legacy quad's bookkeeping (its textures and staging buffers, by proof number).
    residency: Residency,
    /// The surface's format is one textured drawing supports.
    quad_supported: bool,
    /// A submitted frame's copy waits for its proof: the generation it drew.
    capture_waiting: Option<CaptureWait>,
    notice_given: bool,
    /// The number of the last frame submitted: the next frame is built only once its proof is recorded.
    previous: Option<u64>,
    /// The generation of the frame being drawn.
    frame_generation: Option<u64>,
    checker_resident: bool,
    /// The frame timing: the marks, the GPU timestamps and the presentation feedback (milestone 5.5, issue 6, part B).
    timing: Timing,
}

struct SceneRun<'a> {
    core: Core<'a>,
    scene: Box<dyn SceneObject>,
    boundary: FrameBoundary,
}

fn run(
    shared: &Arc<Shared>,
    setup: &SceneSetup,
    surface: vk::SurfaceKHR,
    hooks: TestHooks,
) -> Outcome {
    let observer = hooks.observer.clone();
    let context = setup.0.context();
    if !context.presents() {
        super::fail_setup(
            shared,
            observer.as_deref(),
            "the context was not made for a surface".to_owned(),
        );
        return Outcome::Proven;
    }
    let queue = match context.take_queue() {
        Ok(queue) => queue,
        Err(error) => {
            super::fail_setup(shared, observer.as_deref(), error.to_string());
            return Outcome::Proven;
        }
    };
    let mut gpu = Gpu::on_context(&context, surface, observer.clone());
    gpu.extent_override.clone_from(&hooks.surface_extent);
    gpu.capture_usage = hooks.capture_frames.is_some();
    let (ledger, image_count) = match device_setup(&mut gpu, &hooks) {
        Ok(made) => made,
        Err(fatal) => {
            super::fail_setup(shared, observer.as_deref(), fatal.0);
            // Setup is partial and nothing was ever submitted: destroy exactly what exists.
            gpu.destroy_all();
            gpu.release_holders();
            return Outcome::Proven;
        }
    };
    let limits = setup.0.limits();
    let (buffer, timing) = match start_objects(shared, setup, &mut gpu, &context, limits) {
        Ok(made) => made,
        Err(message) => {
            super::fail_setup(shared, observer.as_deref(), message);
            gpu.destroy_all();
            gpu.release_holders();
            return Outcome::Proven;
        }
    };
    let mark = texture_marker(shared);
    let mut scene = match setup.0.start(wakes_of(&mark), hooks.scene.tickets.clone()) {
        Ok(scene) => scene,
        Err(error) => {
            super::fail_setup(shared, observer.as_deref(), error.to_string());
            let mut buffer = buffer;
            buffer.destroy();
            gpu.destroy_all();
            gpu.release_holders();
            return Outcome::Proven;
        }
    };
    let bindings = scene.bindings();
    // How the textures are bound, once, to the main thread (the bindless table, or the logged fallback), and a wake as after
    // a report.
    if let Some(notice) = scene.binding_notice() {
        *lock(&shared.binding_notice) = Some(notice);
        shared.wake();
    }
    let mut core = Core {
        shared,
        hooks,
        gpu,
        ledger,
        queue: ManuallyDrop::new(queue),
        context: ManuallyDrop::new(context),
        buffer,
        pipeline: None,
        bindings,
        limits,
        image_count,
        first_reported: false,
        failed: false,
        presented: 0,
        rebuild_wanted: false,
        suboptimal_pending: false,
        creation_backoff: BACKOFF_START,
        creation_retry_at: None,
        creation_failed_epoch: 0,
        observer,
        residency: Residency::default(),
        quad_supported: false,
        capture_waiting: None,
        notice_given: false,
        previous: None,
        frame_generation: None,
        checker_resident: false,
        timing,
    };
    start_pipelines(&mut core);
    let mut run = SceneRun {
        core,
        scene,
        boundary: FrameBoundary::Due,
    };
    run.core.publish();
    run.frames();
    run.teardown()
}

/// The instance buffer and the frame timing, made before the scene starts.
fn start_objects(
    shared: &Shared,
    setup: &SceneSetup,
    gpu: &mut Gpu,
    context: &Arc<VulkanContext>,
    limits: SceneLimits,
) -> Result<(InstanceBuffer, Timing), String> {
    let timing = timing_state(shared, gpu, context)?;
    let buffer = instance_buffer(setup, context, limits).map_err(|error| error.to_string())?;
    Ok((buffer, timing))
}

/// The frame timing for the run: the timestamp queries (when the queue family has valid bits) and the sources the reading will say
/// are unavailable, fixed for the run.
fn timing_state(shared: &Shared, gpu: &mut Gpu, context: &VulkanContext) -> Result<Timing, String> {
    let caps = context.timing_caps();
    gpu.create_timestamps(caps.timestamp_valid_bits)
        .map_err(|(call, code)| format!("{call} failed: {code:?}"))?;
    let timing = Timing::new(
        caps.timestamp_valid_bits,
        caps.timestamp_period,
        caps.display_timing,
    );
    let mut recorder = shared.metrics.recorder();
    recorder.gpu_unavailable = timing.gpu_unavailable();
    recorder.presentation_unavailable = timing.presentation_unavailable();
    drop(recorder);
    Ok(timing)
}

/// The instance buffer for the scene's bounds. Its layout depends on the binding mode the ledger will choose (the bindless record
/// carries the slot, padded to the queried stride alignment), decided by the same pure rows before the buffer is made;
/// `start_pipelines` checks it against what the ledger then did.
fn instance_buffer(
    setup: &SceneSetup,
    context: &Arc<VulkanContext>,
    limits: SceneLimits,
) -> Result<InstanceBuffer, Error> {
    let layout = setup.0.instance_layout()?;
    InstanceBuffer::create(
        context,
        layout,
        limits.max_quads,
        limits.binding_geometry_bytes,
    )
}

/// Creates the pipelines: the scene's, and the legacy quad's. They exist only on a swap chain whose format the textured
/// drawing is exact on; anywhere else scene content is skipped with the once-per-run notice and clearing continues.
fn start_pipelines(core: &mut Core<'_>) {
    core.quad_supported = format_supported(core.gpu.format.format);
    if core.bindings.bindless != core.buffer.layout.has_slot() {
        // The layout was decided before the ledger chose its mode; the two use the same rows, so this is a programming error.
        core.fail(
            "the instance layout does not match the texture binding mode the ledger chose"
                .to_owned(),
        );
        return;
    }
    if core.quad_supported
        && let Some(render_pass) = core.gpu.render_pass
    {
        match ScenePipeline::create(
            &core.gpu.device,
            render_pass,
            core.bindings.layout,
            core.bindings.bindless,
            core.buffer.layout,
        ) {
            Ok(pipeline) => core.pipeline = Some(pipeline),
            Err(error) => core.fail(error.to_string()),
        }
        // The legacy quad's pipeline objects too (the quad is drawn first, by today's path, when a frame carries one).
        if let Err(call_failure) = core
            .gpu
            .create_quad_pipeline(core.hooks.quad_faults.pipeline)
        {
            core.fail(failure(call_failure).0);
        }
    }
}

/// Creates the command pool, the render pass and the fixed pools; sizes them from the surface's image count.
fn device_setup(gpu: &mut Gpu, hooks: &TestHooks) -> Result<(Ledger, u32), Fatal> {
    gpu.create_commands().map_err(failure)?;
    let caps = gpu.capabilities().map_err(failure)?;
    let passed = gpu.create_render_pass(hooks.forced_surface_formats.as_deref());
    if hooks.forced_surface_formats.is_some()
        && let Some(observer) = &gpu.observer
    {
        observer.push(Event::Formats {
            offered: gpu.offered_formats.clone(),
            chosen: gpu.format.format,
        });
    }
    passed.map_err(failure)?;
    let mut image_count = caps.min_image_count + 1;
    if caps.max_image_count != 0 {
        image_count = image_count.min(caps.max_image_count);
    }
    // Derived from the image count: the swap chain's images plus the retired swap chain's.
    let pool = 2 * image_count as usize;
    let pools = gpu.create_pools(pool, pool, POOL_FENCES).map_err(failure)?;
    let slots = pools
        .slots
        .into_iter()
        .map(|(semaphore, fence)| PresentSlot { semaphore, fence })
        .collect();
    Ok((Ledger::new(pools.acquire, slots, pools.fences), image_count))
}

/// The handle that marks "texture work pending" under the slot's mutex and signals the render thread. It holds the
/// presenter weakly.
pub(crate) fn texture_marker(shared: &Arc<Shared>) -> Arc<dyn Fn() + Send + Sync> {
    let weak = Arc::downgrade(shared);
    Arc::new(move || {
        if let Some(shared) = weak.upgrade() {
            let mut slot = lock(&shared.slot);
            slot.texture_work = true;
            drop(slot);
            shared.signal.notify_all();
        }
    })
}

impl Core<'_> {
    fn log(&self, event: Event) {
        if let Some(observer) = &self.observer {
            observer.push(event);
        }
    }

    /// The frame step ended: its marks, counts and presentation record go to the recorder, or, when it handed nothing off, it is
    /// counted skipped. The progress fence is released.
    fn finish_step(&mut self) {
        let handed = self.timing.handed_off.take();
        let marks = self.timing.marks;
        let mut recorder = self.shared.metrics.recorder();
        match handed {
            Some(frame) => {
                recorder.drawn(&marks, frame.present_id, &frame.counters, frame.table_binds);
            }
            None => recorder.not_drawn(marks.t0),
        }
        recorder.end_step();
        drop(recorder);
        self.shared.metrics.changed.notify_all();
    }

    fn publish(&self) {
        if let Some(observer) = &self.observer {
            observer.set_snapshot(self.ledger.snapshot());
        }
    }

    fn fail(&mut self, message: String) {
        if !self.failed {
            self.failed = true;
            self.log(Event::Failed(message.clone()));
            self.shared.report(PresenterReport::Failed { message });
        }
    }

    fn flag(flag: Option<&Arc<std::sync::atomic::AtomicBool>>) -> bool {
        flag.is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    // ---- the facts a frame needs ----

    /// Whether a swap chain must be (re)built for `size` before a frame can be drawn.
    fn needs_rebuild(&self, size: (u32, u32)) -> bool {
        self.rebuild_wanted
            || self
                .gpu
                .current
                .as_ref()
                .is_none_or(|chain| chain.requested != size)
    }

    /// Present capacity: the pools can supply a frame, and no rebuild the frame needs is paused (a retired swap chain
    /// waiting for its proofs) or backing off (before `creation_retry_at`, unless a redraw trigger came).
    fn present_capacity(&self, size: (u32, u32), epoch: u64) -> bool {
        if self.needs_rebuild(size) {
            if self.ledger.retired().is_some() {
                return false;
            }
            if let Some(not_before) = self.creation_retry_at
                && Instant::now() < not_before
                && epoch == self.creation_failed_epoch
            {
                return false;
            }
        }
        self.ledger.can_take_frame()
    }

    /// The gates, read from the ledgers, the slot's size and epoch and the scene each time, never from a remembered edge.
    fn gates(&self, scene: &dyn SceneObject, size: (u32, u32), epoch: u64) -> Gates {
        Gates {
            checker: self.checker_resident,
            compat: scene.frame_gate_open(),
            sized: size.0 != 0 && size.1 != 0,
            capacity: self.present_capacity(size, epoch),
            previous: self
                .previous
                .is_none_or(|number| scene.frame_proven(number)),
        }
    }

    /// GPU work is outstanding: an upload batch, a submitted frame, release work waiting on a fence, or a fence of the
    /// present ledger.
    fn gpu_outstanding(&self, scene: &dyn SceneObject) -> bool {
        scene.gpu_outstanding() || self.ledger.has_pending() || self.ledger.retired().is_some()
    }

    // ---- fences ----

    /// Reads the surface's capabilities for a decision of this thread (a test may inject a fault).
    fn query_capabilities(&mut self) -> Result<vk::SurfaceCapabilitiesKHR, Fatal> {
        let result = match TestHooks::next(&mut self.hooks.capability_query) {
            Some(code) => Err(("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", code)),
            None => self.gpu.capabilities(),
        };
        result.map_err(|(call, code)| {
            if code == vk::Result::ERROR_DEVICE_LOST {
                self.ledger.device_lost();
            }
            failure((call, code))
        })
    }

    /// Whether a `VK_SUBOPTIMAL_KHR` result asks for a rebuild (see the legacy thread's rule).
    fn suboptimal_asks_for_a_rebuild(&mut self) -> Result<bool, Fatal> {
        let Some((requested, extent, transform)) = self
            .gpu
            .current
            .as_ref()
            .map(|chain| (chain.requested, chain.extent, chain.transform))
        else {
            return Ok(false);
        };
        let caps = self.query_capabilities()?;
        Ok(Gpu::target_extent(&caps, requested)
            .is_some_and(|target| target != extent || caps.current_transform != transform))
    }

    /// Looks at every pending fence without waiting, and at the texture ledger's latched frame proofs: what signalled is
    /// freed, and the retired swap chain is destroyed once nothing pending belongs to it. Returns whether anything was
    /// freed.
    fn service_fences(&mut self, scene: &dyn SceneObject) -> Result<bool, Fatal> {
        let mut freed = false;
        let never = Self::flag(self.hooks.fences_never_signal.as_ref());
        if !never {
            let held = Self::flag(self.hooks.present_fences_held.as_ref());
            for (raw, proof) in self.ledger.pending() {
                if held && proof == Proof::Present {
                    continue;
                }
                let fence = vk::Fence::from_raw(raw);
                match self.gpu.fence_status(fence) {
                    Ok(true) => {
                        self.gpu
                            .reset_fence(fence)
                            .map_err(|code| failure(("vkResetFences", code)))?;
                        if let Some((proof, chain)) = self.ledger.fence_signalled(raw) {
                            freed = true;
                            if proof == Proof::Present
                                && let Some(id) = self.timing.presentation_of(raw)
                            {
                                let now = self.shared.metrics.now();
                                self.shared.metrics.recorder().fence_seen(id, now);
                            }
                            self.log(Event::FenceSignalled {
                                proof: match proof {
                                    Proof::Submit => "submit",
                                    Proof::Present => "present",
                                    Proof::AcquireDrain => "acquire-drain",
                                    Proof::RenderDrain => "render-drain",
                                },
                                chain,
                            });
                        }
                    }
                    Ok(false) => {}
                    Err(code) => {
                        if code == vk::Result::ERROR_DEVICE_LOST {
                            self.ledger.device_lost();
                        }
                        return Err(failure(("vkGetFenceStatus", code)));
                    }
                }
            }
        }
        // The GPU time of a proven frame, and whatever feedback the presentation engine has for the presents.
        self.timing
            .poll_gpu(&self.gpu, &self.shared.metrics, |number| {
                scene.frame_proven(number)
            });
        self.timing
            .poll_presentation(&self.gpu, &self.shared.metrics);
        // The frame submits are proven by the texture ledger's latched record, never by a fence of this ledger: a fence
        // handle is reset and reused by a later frame, the record is not.
        for _ in 0..self
            .ledger
            .reap_numbered(&|number| scene.frame_proven(number))
        {
            freed = true;
            self.collect_capture();
            self.log(Event::FenceSignalled {
                proof: "submit",
                chain: None,
            });
        }
        // Retirement is not teardown: a retired swap chain is destroyed only while the rendering is healthy, once every
        // proof of it is in, and never once a stop has been requested.
        let stopping = lock(&self.shared.slot).stop;
        // The legacy quad's retired texture and staging buffer follow the same rule, decided from the ledger's record of the
        // proofs.
        self.reap_quad(stopping);
        if !stopping
            && self.ledger.retired_destroyable().is_some()
            && let Some(chain) = self.gpu.retired.take()
        {
            self.gpu.destroy_chain(&chain);
            self.ledger.retired_destroyed();
            freed = true;
        }
        self.publish();
        Ok(freed)
    }

    // ---- the swap chain ----

    /// Creates the swap chain for `size`, retiring the current one first. `Ok(false)` is a failed creation that will be
    /// retried once its backoff has passed and the retired swap chain is destroyed.
    fn rebuild(&mut self, size: (u32, u32)) -> Result<bool, Fatal> {
        // After a failed creation: not before the backoff has passed, unless a redraw trigger came.
        if let Some(not_before) = self.creation_retry_at
            && Instant::now() < not_before
            && lock(&self.shared.slot).epoch == self.creation_failed_epoch
        {
            return Ok(false);
        }
        let mut old = vk::SwapchainKHR::null();
        if let Some(current) = self.gpu.current.take() {
            let Some(id) = self.ledger.retire_current() else {
                // Not reached: the caller checks that nothing is retired.
                self.gpu.current = Some(current);
                return Ok(false);
            };
            old = current.handle;
            self.gpu.retired = Some(current);
            self.log(Event::ChainRetired(id));
        }
        let id = self.ledger.reserve_chain();
        let created = match TestHooks::next(&mut self.hooks.swapchain) {
            Some(code) => Err(("vkCreateSwapchainKHR", code)),
            None => self.gpu.create_chain(id, size, old, self.image_count),
        };
        match created {
            Ok(chain) => {
                self.ledger.chain_created(id);
                self.gpu.current = Some(chain);
                self.rebuild_wanted = false;
                self.creation_backoff = BACKOFF_START;
                self.creation_retry_at = None;
                self.log(Event::ChainCreated { id, size });
                if let Some(observer) = &self.observer {
                    observer.count_chain();
                }
                Ok(true)
            }
            Err((call, code)) => {
                self.ledger.creation_failed();
                self.log(Event::CreationFailed(code));
                if code == vk::Result::ERROR_DEVICE_LOST {
                    self.ledger.device_lost();
                    return Err(failure((call, code)));
                }
                if matches!(
                    code,
                    vk::Result::ERROR_SURFACE_LOST_KHR | vk::Result::ERROR_NATIVE_WINDOW_IN_USE_KHR
                ) {
                    return Err(failure((call, code)));
                }
                // Anything else is retried, with no limit on the attempts, paced by the backoff: the idle wait's deadline.
                self.creation_retry_at = Some(Instant::now() + self.creation_backoff);
                self.creation_backoff = (self.creation_backoff * 2).min(BACKOFF_MAX);
                self.creation_failed_epoch = lock(&self.shared.slot).epoch;
                Ok(false)
            }
        }
    }

    // ---- a frame ----

    fn hold_acquisition(&self) {
        if let Some(hold) = &self.hooks.hold_acquire {
            let limit = Instant::now() + Duration::from_secs(30);
            while hold.load(Ordering::SeqCst) && Instant::now() < limit {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn step(
        &mut self,
        scene: &mut dyn SceneObject,
        rgb: [f32; 3],
        size: (u32, u32),
    ) -> Result<Step, Fatal> {
        self.service_fences(scene)?;
        if size.0 == 0 || size.1 == 0 {
            return Ok(Step::Suspended);
        }
        let read = scene.read_frame();
        // A frame with a compatibility key draws that quad through the ledger; its legacy quad is not drawn.
        let legacy = VulkanScene::new(read.generation, read.quad.filter(|_| !read.has_compat));
        self.retire_for(Some(&legacy));
        if self.needs_rebuild(size) {
            // A surface that is not drawable (a zero extent after the limits) suspends rendering too.
            let caps = self.query_capabilities()?;
            if Gpu::target_extent(&caps, size).is_none() {
                return Ok(Step::Suspended);
            }
            if self.ledger.retired().is_some() {
                // At most one retired swap chain: the rebuild waits, and size changes meanwhile coalesce to the latest size.
                return Ok(Step::Wait);
            }
            if !self.rebuild(size)? {
                return Ok(Step::Wait);
            }
        }
        if self
            .previous
            .is_some_and(|number| !scene.frame_proven(number))
        {
            return Ok(Step::Wait);
        }
        let prepared = match self.prepare(Some(legacy))? {
            Prep::Ready(prepared) => prepared,
            Prep::Wait => return Ok(Step::Wait),
        };
        if self.hooks.capture_frames.is_some()
            && let Some(extent) = self.gpu.current.as_ref().map(|chain| chain.extent)
        {
            self.gpu.ensure_capture(extent).map_err(failure)?;
        }
        let Some(frame) = self.ledger.take_frame() else {
            return Ok(Step::Wait);
        };
        self.hold_acquisition();
        let mut outcome: Option<Result<Step, Fatal>> = None;
        let began = scene.frame_dyn(&mut |seam| {
            let (result, submission) = self.frame(seam, frame, rgb, prepared);
            outcome = Some(result);
            submission
        });
        if !began {
            self.ledger.return_unused(frame);
            return Ok(Step::Wait);
        }
        outcome.unwrap_or(Ok(Step::Wait))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "acquire, build, record, submit and present are one frame and read best together"
    )]
    fn frame(
        &mut self,
        seam: &mut dyn FrameSeam,
        frame: Frame,
        rgb: [f32; 3],
        prepared: Prepared,
    ) -> (Result<Step, Fatal>, Option<crate::FrameSubmission>) {
        let Some(chain_id) = self.gpu.current.as_ref().map(|chain| chain.id) else {
            self.ledger.return_unused(frame);
            return (Ok(Step::Wait), None);
        };
        // ---- acquire ----
        self.timing.marks.t2 = self.shared.metrics.now();
        let injected = TestHooks::next(&mut self.hooks.acquire);
        let acquired = match injected {
            Some(code) if code != vk::Result::SUBOPTIMAL_KHR => Err(code),
            _ => {
                let Some(chain) = self.gpu.current.as_ref() else {
                    self.ledger.return_unused(frame);
                    return (Ok(Step::Wait), None);
                };
                self.gpu
                    .acquire(
                        chain,
                        vk::Semaphore::from_raw(frame.acquire),
                        ACQUIRE_TIMEOUT_NS,
                    )
                    .map(|(index, suboptimal)| (index, suboptimal || injected.is_some()))
            }
        };
        self.timing.marks.t3 = self.shared.metrics.now();
        let index = match acquired {
            Ok((index, suboptimal)) => {
                self.ledger.acquired(index, chain_id);
                self.log(Event::Acquire { index });
                self.suboptimal_pending |= suboptimal;
                index
            }
            Err(code) => {
                self.log(Event::AcquireError(code));
                self.ledger.return_unused(frame);
                return (
                    match code {
                        vk::Result::ERROR_OUT_OF_DATE_KHR => {
                            self.rebuild_wanted = true;
                            Ok(Step::Wait)
                        }
                        vk::Result::TIMEOUT | vk::Result::NOT_READY => Ok(Step::Wait),
                        vk::Result::ERROR_DEVICE_LOST => {
                            self.ledger.device_lost();
                            Err(failure(("vkAcquireNextImageKHR", code)))
                        }
                        other => Err(failure(("vkAcquireNextImageKHR", other))),
                    },
                    None,
                );
            }
        };
        if self.hooks.abandon_frames > 0 {
            self.hooks.abandon_frames -= 1;
            return (self.abandon(&frame, index).map(|()| Step::Wait), None);
        }
        let Some(extent) = self.gpu.current.as_ref().map(|chain| chain.extent) else {
            return (self.abandon(&frame, index).map(|()| Step::Wait), None);
        };
        // ---- build the instances ----
        // Every instance write below depends on the previous submission being proven: one buffer serves one frame in
        // flight. The gate checked it; it is read once more at the write, and a frame whose previous submission is not
        // proven writes nothing.
        let previous_proven = self.previous.is_none_or(|number| seam.proven(number));
        if !previous_proven {
            return (self.abandon(&frame, index).map(|()| Step::Wait), None);
        }
        // The previous frame is proven: its GPU time is read now, before this frame reuses the queries.
        self.timing
            .poll_gpu(&self.gpu, &self.shared.metrics, |number| {
                seam.proven(number)
            });
        let supported = self.pipeline.is_some();
        let built = if supported {
            seam.build(
                (extent.width, extent.height),
                &self.buffer,
                self.limits.max_quads,
            )
        } else {
            self.skip_notice();
            None
        };
        if let Some(builds) = &self.hooks.scene.builds
            && let Some(built) = &built
        {
            lock(builds).push(BuildRecord {
                counters: built.counters,
                previous_proven,
                compat: built.compat,
            });
        }
        if let Some(built) = &built {
            self.timing.built = Some((built.counters, self.bindings.bindless));
        }
        if built.as_ref().is_some_and(|built| built.gated) {
            // Held back by the initial gate: nothing is drawn, the ticket ends not submitted and the image goes back. The
            // want stays (the step waits); the gate parks it until the compatibility key has something to draw.
            return (self.abandon(&frame, index).map(|()| Step::Gated), None);
        }
        if let Some(built) = &built
            && let Err(error) = self
                .buffer
                .flush(usize::try_from(built.counters.instances).unwrap_or(usize::MAX))
        {
            let fatal = Fatal(error.to_string());
            return (self.abandon(&frame, index).and(Err(fatal)), None);
        }
        // ---- record ----
        self.timing.marks.t4 = self.shared.metrics.now();
        let stamps = self.timing.next_queries(self.gpu.has_timestamps());
        let recorded = {
            let draw = match (&built, &self.pipeline) {
                (Some(built), Some(pipeline)) => Some(SceneDraw {
                    pipeline,
                    layout: self.bindings.layout,
                    instances: self.buffer.handle(),
                    batches: seam.batches(),
                    checker: seam.checker_set(),
                    table: seam.table(),
                    samplers: self.bindings.samplers,
                    filter: built.filter,
                }),
                _ => None,
            };
            match self.gpu.current.as_ref() {
                Some(chain) => self.gpu.record_scene_frame(
                    chain,
                    index,
                    rgb,
                    prepared.draw.map(|draw| (draw.params, draw.upload)),
                    draw.as_ref(),
                    FrameExtras {
                        hold: self.hooks.scene.frame_event,
                        stamps,
                    },
                ),
                None => Err((
                    "vkBeginCommandBuffer",
                    vk::Result::ERROR_INITIALIZATION_FAILED,
                )),
            }
        };
        if let Err(call_failure) = recorded {
            let fatal = failure(call_failure);
            return (self.abandon(&frame, index).and(Err(fatal)), None);
        }
        // ---- submit, through the ticket ----
        let Some(cmd) = self.gpu.cmd else {
            let fatal = Fatal("no command buffer".to_owned());
            return (self.abandon(&frame, index).and(Err(fatal)), None);
        };
        if let Some(code) = TestHooks::next(&mut self.hooks.submit) {
            self.queue.inject_submit_error(Some(code));
        }
        let submitted = seam.submit(
            &mut self.queue,
            cmd,
            &[(
                vk::Semaphore::from_raw(frame.acquire),
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            )],
            &[vk::Semaphore::from_raw(frame.slot.semaphore)],
        );
        let submission = match submitted {
            Ok(submission) => submission,
            Err(LedgerError::Native(message)) if message.contains("ERROR_DEVICE_LOST") => {
                self.ledger.device_lost();
                return (Err(Fatal(message)), None);
            }
            Err(error) => {
                let message = match error {
                    LedgerError::Native(message) => message,
                    other => format!("the frame could not be submitted: {other:?}"),
                };
                let fatal = Fatal(message);
                return (self.abandon(&frame, index).and(Err(fatal)), None);
            }
        };
        self.ledger.submitted_numbered(&frame, submission.number());
        self.timing.submitted(submission.number(), stamps);
        self.previous = Some(submission.number());
        self.quad_submitted(submission.number(), prepared);
        self.log(Event::Submit);
        // ---- present ----
        (self.present(&frame, index, rgb), Some(submission))
    }

    /// Records the format notice, once per run, and wakes the main loop as after a report.
    fn skip_notice(&mut self) {
        if self.notice_given {
            return;
        }
        self.notice_given = true;
        let format = self.gpu.format.format;
        *lock(&self.shared.notice) = Some(QuadSkipped { format });
        self.log(Event::QuadSkipped(format));
        self.shared.wake();
    }

    /// An empty submit that waits on `semaphore` to consume its pending signal, with `fence`, through the queue.
    fn drain(&mut self, semaphore: vk::Semaphore, fence: vk::Fence) -> Result<(), vk::Result> {
        let waits = [semaphore];
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let info = vk::SubmitInfo::default()
            .wait_semaphores(&waits)
            .wait_dst_stage_mask(&stages);
        self.queue.submit(&info, fence)
    }

    /// An image acquired and not submitted: released with `vkReleaseSwapchainImagesEXT`, and its acquire semaphore's
    /// pending signal consumed by an empty submit that waits on it.
    fn abandon(&mut self, frame: &Frame, index: u32) -> Result<(), Fatal> {
        self.suboptimal_pending = false;
        if let Some(chain) = self.gpu.current.as_ref() {
            match self.gpu.release_image(chain, index) {
                Ok(()) => self.log(Event::ReleaseImages),
                Err(code) => {
                    if code == vk::Result::ERROR_DEVICE_LOST {
                        self.ledger.device_lost();
                    }
                    return Err(failure(("vkReleaseSwapchainImagesEXT", code)));
                }
            }
        }
        match self.drain(
            vk::Semaphore::from_raw(frame.acquire),
            vk::Fence::from_raw(frame.spare_fence),
        ) {
            Ok(()) => {
                self.log(Event::Drain { what: "acquire" });
                self.ledger.abandoned(frame);
                Ok(())
            }
            Err(code) => {
                if code == vk::Result::ERROR_DEVICE_LOST {
                    self.ledger.device_lost();
                }
                Err(failure(("vkQueueSubmit", code)))
            }
        }
    }

    fn present(&mut self, frame: &Frame, index: u32, rgb: [f32; 3]) -> Result<Step, Fatal> {
        let injected = TestHooks::next(&mut self.hooks.present);
        let semaphore = vk::Semaphore::from_raw(frame.slot.semaphore);
        let fence = vk::Fence::from_raw(frame.slot.fence);
        let chain_id = self.gpu.current.as_ref().map_or(0, |chain| chain.id);
        // The presentation feedback is keyed by the present's number (when presentation timing is on).
        let present_id = self.timing.present_id(frame.slot.fence);
        let real = |core: &mut Self| match core.gpu.current.as_ref() {
            Some(chain) => {
                core.gpu
                    .present_on(&mut core.queue, chain, index, semaphore, fence, present_id)
            }
            None => Err(vk::Result::ERROR_SURFACE_LOST_KHR),
        };
        let result: Result<bool, vk::Result> = match (injected, self.gpu.current.is_some()) {
            (_, false) => Err(vk::Result::ERROR_SURFACE_LOST_KHR),
            // A queued error: the real present runs first, so its semaphore wait and fence are queued, and then the ledger
            // sees the injected result.
            (
                Some(
                    code @ (vk::Result::SUBOPTIMAL_KHR
                    | vk::Result::ERROR_OUT_OF_DATE_KHR
                    | vk::Result::ERROR_SURFACE_LOST_KHR),
                ),
                true,
            ) => match (real(self), code) {
                (Err(real), _) if real == vk::Result::ERROR_DEVICE_LOST => Err(real),
                (_, vk::Result::SUBOPTIMAL_KHR) => Ok(true),
                (_, other) => Err(other),
            },
            // Nothing queued: the real call is not made.
            (Some(code), true) => Err(code),
            (None, true) => real(self),
        };
        // The hand-off mark: the present call has returned.
        let handed_at = self.shared.metrics.now();
        let (call, code) = match result {
            Ok(false) => (PresentCall::Success, vk::Result::SUCCESS),
            Ok(true) => (PresentCall::Suboptimal, vk::Result::SUBOPTIMAL_KHR),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                (PresentCall::OutOfDate, vk::Result::ERROR_OUT_OF_DATE_KHR)
            }
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                (PresentCall::SurfaceLost, vk::Result::ERROR_SURFACE_LOST_KHR)
            }
            Err(
                code @ (vk::Result::ERROR_OUT_OF_HOST_MEMORY
                | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
            ) => (PresentCall::OutOfMemory, code),
            Err(code) => {
                self.ledger.device_lost();
                self.log(Event::Present {
                    chain: chain_id,
                    result: code,
                });
                return Err(failure(("vkQueuePresentKHR", code)));
            }
        };
        self.log(Event::Present {
            chain: chain_id,
            result: code,
        });
        match self.ledger.presented(frame, call) {
            Presented::Outstanding { rebuild, fatal } => {
                if let Some(observer) = &self.observer {
                    observer.count_present();
                }
                if fatal {
                    return Err(Fatal(
                        "the surface was lost (VK_ERROR_SURFACE_LOST_KHR)".to_owned(),
                    ));
                }
                if matches!(call, PresentCall::Success | PresentCall::Suboptimal) {
                    self.hand_off(handed_at, present_id);
                    self.drawn(rgb);
                }
                if rebuild && call == PresentCall::OutOfDate {
                    self.rebuild_wanted = true;
                }
                let suboptimal =
                    std::mem::take(&mut self.suboptimal_pending) || call == PresentCall::Suboptimal;
                if suboptimal && self.suboptimal_asks_for_a_rebuild()? {
                    self.rebuild_wanted = true;
                }
                Ok(Step::Drawn {
                    again: call == PresentCall::OutOfDate || self.rebuild_wanted,
                })
            }
            Presented::NeedsDrain => {
                match self.drain(semaphore, vk::Fence::from_raw(frame.spare_fence)) {
                    Ok(()) => {
                        self.log(Event::Drain {
                            what: "render-finished",
                        });
                        self.ledger.drain_render(frame.spare_fence);
                    }
                    Err(drain) => {
                        if drain == vk::Result::ERROR_DEVICE_LOST {
                            self.ledger.device_lost();
                        }
                    }
                }
                Err(Fatal(format!("the present failed: {code:?}")))
            }
        }
    }

    /// The frame was handed to the presentation engine at `handed_at`: its counts and present number go to the step's record.
    fn hand_off(&mut self, handed_at: u64, present_id: Option<u32>) {
        if let Some((counters, bindless)) = self.timing.built.take() {
            self.timing.handed(
                handed_at,
                HandedOff {
                    counters,
                    table_binds: u64::from(bindless && counters.instances != 0),
                    present_id,
                },
            );
        }
    }

    /// A frame was queued for presentation.
    fn drawn(&mut self, rgb: [f32; 3]) {
        self.presented += 1;
        lock(&self.shared.scene_stats).frames += 1;
        if let Some(generation) = self.frame_generation {
            // Value first, flag second: a reader that sees the flag sees a generation.
            self.shared
                .drawn_generation
                .store(generation, Ordering::Release);
            self.shared.drawn_any.store(true, Ordering::Release);
        }
        if let Some(frames) = &self.hooks.frames {
            frames.fetch_add(1, Ordering::SeqCst);
        }
        if let Some(log) = &self.hooks.clear_log {
            lock(log).push(rgb);
        }
        if !self.first_reported {
            self.first_reported = true;
            self.shared.report(PresenterReport::FirstFrameQueued);
        }
        if self
            .hooks
            .panic_after_frames
            .is_some_and(|limit| self.presented >= limit)
        {
            unwind::forced_panic(
                self.hooks.forced_panic_payload,
                "a forced render thread panic",
            );
        }
    }
}

/// What one ordinary pass found.
struct PassResult {
    /// Runnable work is left: the pass was capped, or the driver or ledger said so.
    more: bool,
    /// Something completed or was reported in this pass.
    progressed: bool,
}

impl SceneRun<'_> {
    /// One non-blocking look at the checker's upload. Resident opens the gate to frames; a failure is the presenter's one
    /// `Failed` report, and nothing is drawn.
    fn poll_checker(&mut self) {
        if self.core.checker_resident || self.core.failed {
            return;
        }
        match self.scene.checker() {
            CheckerProgress::Pending => {}
            CheckerProgress::Resident => {
                self.core.checker_resident = true;
                let mut stats = lock(&self.core.shared.scene_stats);
                stats.checker_resident = true;
                stats.checker_bytes = self.scene.baseline_bytes();
            }
            CheckerProgress::Failed(reason) => self.core.fail(reason),
        }
    }

    /// Publishes the upload store's cumulative counters and byte high-water marks into the scene stats.
    fn publish_uploads(&self) {
        let counts = self.scene.uploads();
        let mut stats = lock(&self.core.shared.scene_stats);
        let checker = (stats.checker_resident, stats.checker_bytes, stats.frames);
        *stats = SceneStats {
            frames: checker.2,
            textures_created: counts.textures_created,
            textures_ready: counts.adopted,
            first_ready_ns: counts.first_adopted_ns,
            last_ready_ns: counts.last_adopted_ns,
            upload_batches_submitted: counts.uploads_submitted,
            upload_batches_completed: counts.uploads_completed,
            texture_bytes_peak: counts.texture_bytes_peak,
            staging_bytes_peak: counts.staging_bytes_peak,
            checker_resident: checker.0,
            checker_bytes: checker.1,
            bindless_slots: self.scene.bindless_slots(),
        };
    }

    /// One pass over the texture scene at `boundary`: `pump`, then the presenter's own consumption of a `GpuFailure` (a
    /// `Failed` report, queued and woken **before** any driver callback runs), then the driver with the remaining logical
    /// events. A pass that reports runnable work left marks texture work pending again, and committed revisions waiting for
    /// a frame boundary make a frame wanted.
    ///
    /// The `Ending` pass belongs to the teardown, whose proof is still open: it only *observes* (no submit, poll, reap or
    /// release) and hands the driver no events.
    fn texture_pass(&mut self, boundary: Boundary) -> PassResult {
        let (failure, pumped_more, progressed) = if boundary == Boundary::Ending {
            (self.scene.observe(), false, false)
        } else {
            let pumped = self.scene.pump(&mut self.core.queue);
            (pumped.failure, pumped.more, pumped.progressed)
        };
        if let Some(message) = failure {
            self.core.fail(message);
        }
        let outcome = self.scene.drive(boundary);
        self.publish_uploads();
        if self.core.failed || boundary == Boundary::Ending {
            return PassResult {
                more: false,
                progressed,
            };
        }
        let committed = self.scene.has_committed();
        let more = pumped_more || outcome.more;
        let mut slot = lock(&self.core.shared.slot);
        if more {
            slot.texture_work = true;
        }
        if committed {
            slot.dirty = true;
        }
        PassResult { more, progressed }
    }

    /// Waits for work or the stop. `Some` is a snapshot of what to draw; `None` is the stop.
    ///
    /// Runnable work is never delayed: a capped pass, a closed envelope, a completion observed, or a runnable frame (wanted
    /// and every gate open) makes the thread loop again with no wait. A want that fails a gate is parked, and the thread
    /// waits only when nothing is runnable (see [`idle_deadline`]).
    fn next_work(&mut self) -> Option<([f32; 3], (u32, u32))> {
        loop {
            if lock(&self.core.shared.slot).stop {
                return None;
            }
            // A requested measurement window is adopted at this boundary.
            let metrics = &self.core.shared.metrics;
            if metrics.check.swap(false, Ordering::AcqRel) {
                let now = metrics.now();
                metrics.recorder().windows.boundary(now);
                metrics.changed.notify_all();
            }
            let mut runnable = false;
            if let Err(Fatal(message)) = self.core.service_fences(&*self.scene) {
                self.core.fail(message);
            }
            // The ordinary pass: texture work was marked, GPU work is outstanding (completions are polled, there is no
            // callback), or a closed envelope waits for its submit.
            let marked = {
                let mut slot = lock(&self.core.shared.slot);
                std::mem::take(&mut slot.texture_work)
            };
            if !self.core.failed
                && (marked || self.scene.gpu_outstanding() || self.scene.envelope_ready())
            {
                let pass = self.texture_pass(Boundary::Pass);
                runnable |= pass.more || pass.progressed;
                if let Err(Fatal(message)) = self.core.service_fences(&*self.scene) {
                    self.core.fail(message);
                }
            }
            self.poll_checker();
            {
                let slot = lock(&self.core.shared.slot);
                if slot.stop {
                    return None;
                }
                let gates = self.core.gates(&*self.scene, slot.size, slot.epoch);
                if frame_runnable(slot.dirty, self.core.failed, gates) {
                    drop(slot);
                    if let Some(gate) = &self.core.hooks.gate
                        && !gate.take(|| !lock(&self.core.shared.slot).stop)
                    {
                        return None;
                    }
                    let mut slot = lock(&self.core.shared.slot);
                    if slot.stop {
                        return None;
                    }
                    if slot.dirty {
                        slot.dirty = false;
                        return Some((slot.clear, slot.size));
                    }
                    continue;
                }
            }
            if runnable || self.idle() {
                continue;
            }
            return None;
        }
    }

    /// The idle decision, with nothing runnable. `true`: look again at once (a completion landed, or a want arrived since
    /// the look); `false`: the stop. Otherwise waits, and returns `true` when the wait ended.
    fn idle(&mut self) -> bool {
        // Nothing is runnable. A test may hold the thread here, between the unlocked look and the locked recheck.
        if let Some(pause) = &self.core.hooks.scene.pause_before_wait {
            pause.wait();
        }
        // A last non-blocking look, so a completion that landed since the pass above is not waited for: with GPU
        // work outstanding, a completion already signalled at the idle decision makes the thread loop, not wait.
        if !self.core.failed && self.core.gpu_outstanding(&*self.scene) {
            let pass = self.texture_pass(Boundary::Pass);
            let freed = match self.core.service_fences(&*self.scene) {
                Ok(freed) => freed,
                Err(Fatal(message)) => {
                    self.core.fail(message);
                    false
                }
            };
            if pass.more || pass.progressed || freed {
                return true;
            }
        }
        let slot = lock(&self.core.shared.slot);
        if slot.stop {
            return false;
        }
        // The sticky predicate again, **under the mutex the wait releases**: a publication that landed since the look
        // above has set `dirty` under it, so either this read sees it or its notify comes after the thread waits.
        let gates = self.core.gates(&*self.scene, slot.size, slot.epoch);
        if frame_runnable(slot.dirty, self.core.failed, gates)
            || (slot.texture_work && !self.core.failed)
            || self.core.shared.metrics.check.load(Ordering::Acquire)
        {
            return true;
        }
        let facts = IdleFacts {
            presentation_pending: self
                .core
                .timing
                .presentation_pending(&self.core.shared.metrics),
            gpu_outstanding: self.core.gpu_outstanding(&*self.scene),
            retry_at: (slot.dirty && !self.core.failed)
                .then_some(self.core.creation_retry_at)
                .flatten()
                .filter(|_| self.core.needs_rebuild(slot.size)),
        };
        let decision = idle_deadline(Instant::now(), facts);
        if let Some(waits) = &self.core.hooks.scene.waits {
            waits.entered(decision.map_or(WaitKind::Unbounded, |(_, kind)| kind));
        }
        let _released = match decision {
            None => self
                .core
                .shared
                .signal
                .wait(slot)
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Some((deadline, _)) => {
                self.core
                    .shared
                    .signal
                    .wait_timeout(slot, deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0
            }
        };
        true
    }

    /// Committed revisions waiting for a frame boundary make a frame wanted, so adoption never waits for an unrelated
    /// redraw.
    fn want_frame_for_committed(&mut self) {
        if !self.core.failed && self.scene.has_committed() {
            lock(&self.core.shared.slot).dirty = true;
        }
    }

    fn frames(&mut self) {
        while let Some((rgb, size)) = self.next_work() {
            if let Some(waits) = &self.core.hooks.scene.waits {
                waits.attempted();
            }
            // The step's first mark: the latest frame is taken. The progress fence holds the windows open until it is recorded.
            let t0 = self.core.shared.metrics.now();
            self.core.timing.begin(t0);
            self.core.shared.metrics.recorder().begin_step(t0);
            // The frame boundary: the pass immediately before the frame is built, where the driver adopts. A step that is
            // attempted again keeps its boundary: texture work that woke the thread meanwhile gets an ordinary pass first.
            if self.boundary == FrameBoundary::Due {
                self.texture_pass(Boundary::Frame);
                self.boundary = FrameBoundary::Passed;
            }
            self.core.timing.marks.t1 = self.core.shared.metrics.now();
            if self.core.failed {
                // A failure the pass found: nothing more is drawn.
                self.core.finish_step();
                continue;
            }
            let outcome = self.core.step(&mut *self.scene, rgb, size);
            self.core.finish_step();
            match outcome {
                Ok(Step::Drawn { again }) => {
                    self.boundary = FrameBoundary::Due;
                    if again {
                        lock(&self.core.shared.slot).dirty = true;
                    }
                    self.want_frame_for_committed();
                }
                Ok(Step::Wait) => {
                    // The redraw stays pending. No sleep: a want that fails a gate is parked until its facts change, and
                    // one that does not is attempted again at once.
                    if !self.core.failed {
                        lock(&self.core.shared.slot).dirty = true;
                    }
                }
                Ok(Step::Gated) => {
                    self.boundary = FrameBoundary::Due;
                    if !self.core.failed {
                        lock(&self.core.shared.slot).dirty = true;
                    }
                }
                Ok(Step::Suspended) => self.boundary = FrameBoundary::Due,
                Err(Fatal(message)) => {
                    self.boundary = FrameBoundary::Due;
                    self.core.fail(message);
                }
            }
            // A frame can free what it held: the release work it queued is run by an ordinary pass now.
            if !self.core.failed && self.scene.has_release_work() {
                lock(&self.core.shared.slot).texture_work = true;
            }
            self.core.publish();
        }
    }

    // ---- teardown ----

    /// After the stop: the driver's last pass, then a look at **every** proof - the present ledger's fences, the texture
    /// ledger's frame proofs, uploads and worker - without releasing anything, again and again until all are complete or the
    /// one absolute stop deadline passes. Only when all are complete is anything destroyed, in dependency order (the
    /// pipeline and the instance buffer, the texture ledger and store, the thread's objects, the context last): `Proven`.
    /// Otherwise nothing is destroyed and every holder, the context included, is kept.
    fn teardown(mut self) -> Outcome {
        if self.core.hooks.panic_at_stop {
            unwind::forced_panic(
                self.core.hooks.forced_panic_payload,
                "a forced stop-time panic",
            );
        }
        let deadline = lock(&self.core.shared.slot)
            .stop_deadline
            .unwrap_or_else(Instant::now);
        // 1. The driver's last pass; no ticket exists between frame steps.
        self.texture_pass(Boundary::Ending);
        loop {
            // 2. Look at the store and ledger (this stops offering) and at the present ledger. A completion error either
            //    side saw is a failure, reported before anything is released.
            if let Some(message) = self.scene.observe() {
                self.core.fail(message);
            }
            let look = Instant::now() + Duration::from_millis(2);
            let scene_outstanding = self.scene.prove(look.min(deadline));
            if let Err(Fatal(message)) = self.core.service_fences(&*self.scene) {
                self.core.fail(message);
            }
            if scene_outstanding.is_none() && self.core.ledger.proven() {
                // 3. All complete. A failure the ledger latched while it was being proven is reported first.
                if let Some(message) = self.scene.observe() {
                    self.core.fail(message);
                }
                self.core.publish();
                self.publish_uploads();
                return self.release();
            }
            if self.core.ledger.is_lost() {
                self.core.publish();
                return self.unproven(format!(
                    "{}; every object, the instance and the loader were kept",
                    self.core.ledger.describe_outstanding()
                ));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.core.publish();
                let store = scene_outstanding.map_or_else(String::new, |reason| {
                    format!(" and the texture store or ledger incomplete ({reason})")
                });
                return self.unproven(format!(
                    "the stop deadline passed with {}{store}; every object, the texture store and ledger, the instance and the loader were kept",
                    self.core.ledger.describe_outstanding()
                ));
            }
            let never = Core::flag(self.core.hooks.fences_never_signal.as_ref());
            let held = Core::flag(self.core.hooks.present_fences_held.as_ref());
            let fences: Vec<vk::Fence> = self
                .core
                .ledger
                .pending()
                .into_iter()
                .filter(|(_, proof)| !(held && *proof == Proof::Present))
                .map(|(raw, _)| vk::Fence::from_raw(raw))
                .collect();
            if never || fences.is_empty() {
                thread::sleep(left.min(Duration::from_millis(1)));
            } else {
                let timeout = u64::try_from(left.min(Duration::from_millis(2)).as_nanos())
                    .unwrap_or(2_000_000);
                match self.core.gpu.wait_any(&fences, timeout) {
                    Ok(()) | Err(vk::Result::TIMEOUT) => {}
                    Err(code) => {
                        if code == vk::Result::ERROR_DEVICE_LOST {
                            self.core.ledger.device_lost();
                        }
                        self.core.fail(format!("vkWaitForFences failed: {code:?}"));
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
    }

    /// The complete proof is in: releases in dependency order.
    fn release(mut self) -> Outcome {
        if let Some(mut pipeline) = self.core.pipeline.take() {
            pipeline.destroy(&self.core.gpu.device);
        }
        self.core.buffer.destroy();
        if let Err(reason) = self.scene.release() {
            return self.unproven(format!(
                "the texture store and ledger refused their release ({reason}); the device, queue, store and ledger were kept"
            ));
        }
        self.core.gpu.destroy_all();
        self.core.gpu.release_holders();
        // The scene (released), then the queue and, last, the context.
        let SceneRun { core, scene, .. } = self;
        drop(scene);
        let Core { queue, context, .. } = core;
        drop(ManuallyDrop::into_inner(queue));
        drop(ManuallyDrop::into_inner(context));
        Outcome::Proven
    }

    /// The unproven end: nothing is destroyed, and the store, ledger and context are kept to the process end (a leak by
    /// design).
    fn unproven(&mut self, reason: String) -> Outcome {
        self.publish_uploads();
        Outcome::Unproven(reason)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Gates, IdleFacts, frame_runnable, idle_deadline};
    use crate::hooks::WaitKind;

    fn open() -> Gates {
        Gates {
            checker: true,
            compat: true,
            sized: true,
            capacity: true,
            previous: true,
        }
    }

    #[test]
    fn an_idle_thread_waits_without_a_bound_only_when_nothing_is_outstanding_and_no_retry_is_pending()
     {
        let now = Instant::now();
        assert_eq!(
            idle_deadline(
                now,
                IdleFacts {
                    gpu_outstanding: false,
                    retry_at: None,
                    presentation_pending: false
                }
            ),
            None
        );
    }

    #[test]
    fn outstanding_gpu_work_bounds_the_wait_at_one_millisecond() {
        let now = Instant::now();
        assert_eq!(
            idle_deadline(
                now,
                IdleFacts {
                    gpu_outstanding: true,
                    retry_at: None,
                    presentation_pending: false
                }
            ),
            Some((now + Duration::from_millis(1), WaitKind::GpuPoll))
        );
    }

    #[test]
    fn a_pending_retry_is_the_deadline_when_it_is_earliest_and_the_poll_when_it_is_not() {
        let now = Instant::now();
        let soon = now + Duration::from_micros(300);
        let late = now + Duration::from_millis(50);
        assert_eq!(
            idle_deadline(
                now,
                IdleFacts {
                    gpu_outstanding: false,
                    retry_at: Some(late),
                    presentation_pending: false,
                }
            ),
            Some((late, WaitKind::RetryDeadline)),
            "a parked rebuild with nothing outstanding waits for its retry"
        );
        assert_eq!(
            idle_deadline(
                now,
                IdleFacts {
                    gpu_outstanding: true,
                    retry_at: Some(soon),
                    presentation_pending: false,
                }
            ),
            Some((soon, WaitKind::RetryDeadline))
        );
        assert_eq!(
            idle_deadline(
                now,
                IdleFacts {
                    gpu_outstanding: true,
                    retry_at: Some(late),
                    presentation_pending: false,
                }
            ),
            Some((now + Duration::from_millis(1), WaitKind::GpuPoll))
        );
    }

    #[test]
    fn a_retry_already_due_gives_a_deadline_in_the_past_so_the_wait_returns_at_once() {
        let now = Instant::now();
        let due = now.checked_sub(Duration::from_millis(5)).unwrap_or(now);
        let (deadline, kind) = idle_deadline(
            now,
            IdleFacts {
                gpu_outstanding: false,
                retry_at: Some(due),
                presentation_pending: false,
            },
        )
        .expect("a deadline");
        assert_eq!(kind, WaitKind::RetryDeadline);
        assert!(deadline <= now);
    }

    /// Negative control: dropping the retry from the calculation strands a parked rebuild in an unbounded wait.
    #[test]
    fn negative_control_a_calculation_without_the_retry_waits_without_a_bound_for_a_parked_rebuild()
    {
        fn without_retry(now: Instant, facts: IdleFacts) -> Option<Instant> {
            facts
                .gpu_outstanding
                .then(|| now + Duration::from_millis(1))
        }
        let now = Instant::now();
        let facts = IdleFacts {
            gpu_outstanding: false,
            retry_at: Some(now + Duration::from_millis(50)),
            presentation_pending: false,
        };
        assert_eq!(
            without_retry(now, facts),
            None,
            "the mutant never wakes for the retry"
        );
        assert!(
            idle_deadline(now, facts).is_some(),
            "the real calculation does"
        );
    }

    /// Negative control: an unconditional 1 ms wait whenever a fence is outstanding would bound a thread that has
    /// runnable work; the real rule bounds only an idle one, and runnable work never reaches it.
    #[test]
    fn a_capped_pass_is_runnable_work_and_the_idle_rule_is_not_asked() {
        // `next_work` loops again when `runnable`; `idle_deadline` is only reached with nothing runnable. The facts of a
        // thread with a closed envelope and no outstanding GPU work would otherwise wait without a bound.
        let facts = IdleFacts {
            gpu_outstanding: false,
            retry_at: None,
            presentation_pending: false,
        };
        assert_eq!(idle_deadline(Instant::now(), facts), None);
    }

    #[test]
    fn a_frame_is_runnable_only_when_wanted_not_failed_and_every_gate_is_open() {
        assert!(frame_runnable(true, false, open()));
        assert!(!frame_runnable(false, false, open()), "not wanted");
        assert!(!frame_runnable(true, true, open()), "failed");
        for closed in [
            Gates {
                checker: false,
                ..open()
            },
            Gates {
                compat: false,
                ..open()
            },
            Gates {
                sized: false,
                ..open()
            },
            Gates {
                capacity: false,
                ..open()
            },
            Gates {
                previous: false,
                ..open()
            },
        ] {
            assert!(
                !frame_runnable(true, false, closed),
                "{closed:?} parks the want"
            );
        }
    }

    /// Negative control: treating "wanted with the previous submission proven" as runnable attempts a frame while the
    /// checker is pending, again and again.
    #[test]
    fn negative_control_wanted_with_the_previous_proven_alone_is_runnable_in_the_mutant_only() {
        fn mutant(dirty: bool, failed: bool, gates: Gates) -> bool {
            dirty && !failed && gates.previous
        }
        let checker_pending = Gates {
            checker: false,
            ..open()
        };
        assert!(
            mutant(true, false, checker_pending),
            "the mutant attempts the frame"
        );
        assert!(
            !frame_runnable(true, false, checker_pending),
            "the real rule parks it"
        );
    }
}
