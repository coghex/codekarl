//! The render thread: setup, the frame loop with the present-lifetime ledger, and the proven teardown.
//!
//! Every device-level object lives here only, in [`Gpu`](crate::gpu::Gpu), which has no `Drop`; the thread's
//! holds on the instance and the loader sit in `ManuallyDrop`, so a thread that ends by unwinding releases
//! nothing. Only [`Run::teardown`] destroys, and only once the ledger proved every operation finished.

use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ash::vk;
use ash::vk::Handle as _;

use crate::draw::{QuadParams, QuadTexture, Tally, device_checks, upload_bytes};
use crate::gpu::{self, Gpu};
use crate::gpu_quad::{format_supported, release_staging, release_texture};
use crate::hooks::{Event, Observer, POLL, TestHooks};
use crate::instance::InstanceInner;
use crate::ledger::{Frame, Ledger, PresentCall, PresentSlot, Presented, Proof};
use crate::loader::Loader;
use crate::quad::VulkanScene;
use crate::readback::ReadbackHooks;
use crate::residency::{Identity, Owner, Replaced, Residency, Retirement};
use crate::shared::{CleanupResult, PresenterReport, Shared, lock};
use crate::slot::{QuadSkipped, VulkanQuadSlot};
use crate::unwind;

/// How long one acquire may block: a bounded wait, so a stop is noticed (the stop deadline is absolute).
const ACQUIRE_TIMEOUT_NS: u64 = 50_000_000;
/// The first wait after a failed swap chain creation; it doubles with each failure up to the maximum and
/// is reset by a success.
const BACKOFF_START: Duration = Duration::from_millis(50);
/// The longest wait between swap chain creation attempts.
const BACKOFF_MAX: Duration = Duration::from_secs(1);
/// Fences in the pool beyond the present slots': a submit fence and a spare per frame in flight, with room
/// for the drains.
const POOL_FENCES: usize = 6;

mod quad;
mod scene;

pub(crate) use scene::{scene_main, texture_marker};

/// A failure that ends the drawing: reported once as `Failed`.
struct Fatal(String);

fn failure((call, code): (&'static str, vk::Result)) -> Fatal {
    Fatal(format!("{call} failed: {code:?}"))
}

/// What a frame step did.
enum Step {
    /// A frame was queued; `again` asks for another (the present was out of date).
    Drawn { again: bool },
    /// Nothing could be drawn now (a pool or swap chain is busy); the redraw stays pending.
    Wait,
    /// The window's pixel size is zero: nothing is drawn and no swap chain is created.
    Suspended,
}

/// How the thread ended.
pub(crate) enum Outcome {
    Proven,
    Unproven(String),
}

/// The thread's entry point: runs the presenter and publishes its cleanup result. A panic is caught here
/// and gives `Unproven` with every holder kept.
pub(crate) fn render_main(
    shared: &Arc<Shared>,
    inner: Arc<InstanceInner>,
    loader: Loader,
    surface: vk::SurfaceKHR,
    hooks: TestHooks,
) {
    if let Some(started) = &hooks.thread_started {
        started.store(true, Ordering::SeqCst);
    }
    // The thread's own holds, in `ManuallyDrop`: dropped only after a proven end.
    let mut inner = ManuallyDrop::new(inner);
    let mut loader = ManuallyDrop::new(loader);
    let quads = hooks.quad_slot.clone();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        run(shared, &inner, &loader, surface, hooks)
    }));
    let result = match outcome {
        Ok(Outcome::Proven) => {
            // SAFETY: the end is proven; nothing uses these holds any more, and they are dropped once.
            unsafe { ManuallyDrop::drop(&mut inner) };
            // SAFETY: as above.
            unsafe { ManuallyDrop::drop(&mut loader) };
            CleanupResult::Proven
        }
        Ok(Outcome::Unproven(reason)) => CleanupResult::Unproven { reason },
        Err(payload) => {
            // Disposed of without unwinding, so the result below is always stored and the wake sent.
            unwind::dispose(payload);
            CleanupResult::Unproven {
                reason: "the render thread panicked; everything it owned was kept".to_owned(),
            }
        }
    };
    if let Some(quads) = quads {
        // The presenter is over: the slot stops waking it (a publish from now on only stores the value).
        quads.clear_wake();
    }
    *lock(&shared.result) = Some(result);
    shared.wake();
}

fn run(
    shared: &Arc<Shared>,
    inner: &Arc<InstanceInner>,
    loader: &Loader,
    surface: vk::SurfaceKHR,
    hooks: TestHooks,
) -> Outcome {
    let observer = hooks.observer.clone();
    let size = lock(&shared.slot).size;
    let selection = match gpu::select(inner, surface, hooks.hide_extension.as_deref()) {
        Ok(selection) => selection,
        Err(message) => {
            fail_setup(shared, observer.as_deref(), message);
            // Nothing has been created, so there is nothing to prove.
            return Outcome::Proven;
        }
    };
    let hook_extent = hooks.surface_extent.clone();
    let mut gpu = match Gpu::create_device(
        Arc::clone(inner),
        loader.clone(),
        surface,
        selection,
        observer.clone(),
    ) {
        Ok(gpu) => gpu,
        Err(failure) => {
            fail_setup(shared, observer.as_deref(), self::failure(failure).0);
            return Outcome::Proven;
        }
    };
    gpu.extent_override = hook_extent;
    gpu.capture_usage = hooks.capture_frames.is_some();
    match setup(&mut gpu, &hooks) {
        Ok((ledger, image_count)) => {
            let quad_supported = format_supported(gpu.format.format);
            let quads = hooks.quad_slot.clone();
            let mut run = Run {
                shared,
                hooks,
                gpu,
                ledger,
                image_count,
                first_reported: false,
                failed: false,
                presented: 0,
                rebuild_wanted: false,
                suboptimal_pending: false,
                creation_backoff: BACKOFF_START,
                creation_retry_at: None,
                creation_failed_epoch: 0,
                last_clear: [0.0; 3],
                observer,
                residency: Residency::default(),
                quads,
                quad_supported,
                notice_given: false,
                frame_generation: None,
                capture_waiting: None,
            };
            run.publish();
            run.frames(size);
            run.teardown()
        }
        Err(fatal) => {
            fail_setup(shared, observer.as_deref(), fatal.0);
            // Setup is partial and nothing was ever submitted: destroy exactly what exists.
            gpu.destroy_all();
            gpu.release_holders();
            Outcome::Proven
        }
    }
}

fn fail_setup(shared: &Shared, observer: Option<&Observer>, message: String) {
    if let Some(observer) = observer {
        observer.push(Event::Failed(message.clone()));
    }
    shared.report(PresenterReport::Failed { message });
}

/// Creates the command pool, the render pass, the quad pipeline (when there is a slot and the format is
/// supported) and the fixed pools; sizes them from the surface's image count.
fn setup(gpu: &mut Gpu, hooks: &TestHooks) -> Result<(Ledger, u32), Fatal> {
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
    if hooks.quad_slot.is_some() && format_supported(gpu.format.format) {
        // Only for a supported format: on any other the quad is never drawn, so an unsupported format cannot
        // fail set-up. A failure here is a `Failed` under the set-up rules (what exists is destroyed).
        gpu.create_quad_pipeline(hooks.quad_faults.pipeline)
            .map_err(failure)?;
    }
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

/// What the render thread keeps while it runs.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the thread's independent state flags, each read on its own"
)]
struct Run<'a> {
    shared: &'a Shared,
    hooks: TestHooks,
    gpu: Gpu,
    ledger: Ledger,
    image_count: u32,
    first_reported: bool,
    failed: bool,
    presented: u64,
    /// The swap chain must be rebuilt: an acquire or present said out of date or suboptimal.
    rebuild_wanted: bool,
    /// An acquire said `VK_SUBOPTIMAL_KHR` for the frame in progress.
    suboptimal_pending: bool,
    /// The wait after the next failed swap chain creation.
    creation_backoff: Duration,
    /// Not before this moment is a failed swap chain creation tried again, unless a redraw trigger came.
    creation_retry_at: Option<Instant>,
    /// The slot's redraw-trigger count when the creation last failed.
    creation_failed_epoch: u64,
    last_clear: [f32; 3],
    observer: Option<Arc<Observer>>,
    /// The quad textures' bookkeeping.
    residency: Residency,
    /// The slot the presenter was started with, if any.
    quads: Option<Arc<VulkanQuadSlot>>,
    /// The surface's format is one textured drawing supports.
    quad_supported: bool,
    /// The format notice was recorded (once per run).
    notice_given: bool,
    /// The generation of the scene the last submitted frame drew.
    frame_generation: Option<u64>,
    /// A submitted frame's copy waits for its submit's proof: the generation it drew.
    capture_waiting: Option<CaptureWait>,
}

/// A frame whose copy waits for its submit's proof.
struct CaptureWait {
    /// The generation of the scene the frame drew.
    generation: Option<u64>,
    /// The number of the frame's submission.
    submission: u64,
}

/// What a frame draws of the quad.
#[derive(Clone, Copy)]
struct Draw {
    params: QuadParams,
    /// The resident texture's upload is recorded in this frame.
    upload: bool,
}

/// What a step prepared for its frame.
#[derive(Clone, Copy, Default)]
struct Prepared {
    /// The generation of the scene the frame draws (`None` without a slot).
    generation: Option<u64>,
    draw: Option<Draw>,
}

enum Prep {
    Ready(Prepared),
    /// The frame waits, its redraw kept.
    Wait,
}

impl Run<'_> {
    fn log(&self, event: Event) {
        if let Some(observer) = &self.observer {
            observer.push(event);
        }
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

    /// Waits for work or the stop. `Some` is a snapshot of what to draw; `None` is the stop.
    fn next_work(&mut self) -> Option<([f32; 3], (u32, u32))> {
        loop {
            let wanted = {
                let slot = lock(&self.shared.slot);
                if slot.stop {
                    return None;
                }
                slot.dirty && !self.failed
            };
            if !wanted {
                let busy = self.ledger.has_pending() || self.ledger.retired().is_some();
                {
                    let slot = lock(&self.shared.slot);
                    if !slot.stop && (!slot.dirty || self.failed) {
                        let _released = if busy {
                            self.shared
                                .signal
                                .wait_timeout(slot, POLL)
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .0
                        } else {
                            self.shared
                                .signal
                                .wait(slot)
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                        };
                    }
                }
                // A test may hold the thread right here, between its wake and the service of the fences, while a
                // swap chain is retired (so a wake for something else is not held).
                if let Some(pause) = &self.hooks.pause_before_idle_service
                    && self.ledger.retired().is_some()
                {
                    pause.wait();
                }
                if let Err(Fatal(message)) = self.service_fences() {
                    self.fail(message);
                }
                continue;
            }
            if let Some(gate) = &self.hooks.gate
                && !gate.take(|| !lock(&self.shared.slot).stop)
            {
                return None;
            }
            let mut slot = lock(&self.shared.slot);
            if slot.stop {
                return None;
            }
            if slot.dirty {
                slot.dirty = false;
                return Some((slot.clear, slot.size));
            }
        }
    }

    fn frames(&mut self, _initial: (u32, u32)) {
        while let Some((rgb, size)) = self.next_work() {
            self.last_clear = rgb;
            match self.step(rgb, size) {
                Ok(Step::Drawn { again }) => {
                    if again {
                        lock(&self.shared.slot).dirty = true;
                    }
                }
                Ok(Step::Wait) => {
                    if !self.failed {
                        lock(&self.shared.slot).dirty = true;
                    }
                    thread::sleep(POLL);
                }
                Ok(Step::Suspended) => {}
                Err(Fatal(message)) => self.fail(message),
            }
            self.publish();
        }
    }

    // ---- fences ----

    /// Whether a test hook's flag reads true.
    fn flag(flag: Option<&Arc<AtomicBool>>) -> bool {
        flag.is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// Reads the surface's capabilities for a decision of this thread (a test may inject a fault). A failure
    /// follows the failure and proof rules: a lost device is recorded in the ledger (nothing is proven or
    /// destroyed again), and the error is a `Failed`.
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

    /// Whether a `VK_SUBOPTIMAL_KHR` result asks for a rebuild: only when the surface's configuration (the
    /// normalised, limit-clamped extent a swap chain would be created with now, and the transform) differs
    /// from the current swap chain's creation parameters. At the parameters it already has, a suboptimal
    /// result does not rebuild (the frame still counts as presented); a later change of the parameters, or an
    /// out-of-date result, does. A surface that is not drawable (a zero extent) does not rebuild either.
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

    /// Looks at every pending fence without waiting: what signalled is freed, and the retired swap chain is
    /// destroyed once nothing pending belongs to it.
    fn service_fences(&mut self) -> Result<(), Fatal> {
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
                            if proof == Proof::Submit {
                                self.collect_capture();
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
        // Retirement is not teardown: a retired swap chain is destroyed only while the rendering is healthy,
        // once every proof of it is in. After a device loss it is kept with everything else (the ledger says
        // so), and so it is once a stop has been requested: this is decided from the stop state itself, read
        // under the lock the request writes it under, at the moment of the decision, whichever of the
        // thread's paths (a step, an idle wake, the teardown) is servicing the fences. Before every proof of
        // every swap chain succeeded, nothing is destroyed.
        let stopping = lock(&self.shared.slot).stop;
        if !stopping
            && self.ledger.retired_destroyable().is_some()
            && let Some(chain) = self.gpu.retired.take()
        {
            self.gpu.destroy_chain(&chain);
            self.ledger.retired_destroyed();
        }
        // The quad's retired texture and staging buffer follow the same rule, decided from the ledger's record of
        // the proofs.
        self.reap_quad(stopping);
        self.publish();
        Ok(())
    }

    // ---- the swap chain ----

    /// Creates the swap chain for `size`, retiring the current one first. `Ok(false)` is a failed creation
    /// that will be retried once the retired swap chain is destroyed.
    fn rebuild(&mut self, size: (u32, u32)) -> Result<bool, Fatal> {
        // After a failed creation: not before the backoff has passed, unless a redraw trigger came (a colour
        // or size change, a redraw request). The thread never retries without waiting.
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
                // The old swap chain, passed as `oldSwapchain`, is retired anyway; nothing is current.
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
                    // The surface cannot recover.
                    return Err(failure((call, code)));
                }
                // Anything else is retried, with no limit on the attempts, paced by the backoff.
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

    fn step(&mut self, rgb: [f32; 3], size: (u32, u32)) -> Result<Step, Fatal> {
        self.service_fences()?;
        if size.0 == 0 || size.1 == 0 {
            return Ok(Step::Suspended);
        }
        let scene = self.read_scene();
        self.retire_for(scene.as_ref());
        let stale = self
            .gpu
            .current
            .as_ref()
            .is_none_or(|chain| chain.requested != size);
        if stale || self.rebuild_wanted {
            // A surface that is not drawable (a zero extent after the limits) suspends rendering too: no
            // swap chain is created for it.
            let caps = self.query_capabilities()?;
            if Gpu::target_extent(&caps, size).is_none() {
                return Ok(Step::Suspended);
            }
            if self.ledger.retired().is_some() {
                // At most one retired swap chain: the rebuild waits, and size changes meanwhile coalesce to
                // the latest size (the slot holds only the latest).
                return Ok(Step::Wait);
            }
            if !self.rebuild(size)? {
                return Ok(Step::Wait);
            }
        }
        if self.ledger.submit_in_flight() {
            return Ok(Step::Wait);
        }
        let prepared = match self.prepare(scene)? {
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
        self.frame(frame, rgb, prepared)
    }

    fn frame(&mut self, frame: Frame, rgb: [f32; 3], prepared: Prepared) -> Result<Step, Fatal> {
        let Some(chain_id) = self.gpu.current.as_ref().map(|chain| chain.id) else {
            self.ledger.return_unused(frame);
            return Ok(Step::Wait);
        };
        // ---- acquire ----
        let injected = TestHooks::next(&mut self.hooks.acquire);
        let acquired = match injected {
            Some(code) if code != vk::Result::SUBOPTIMAL_KHR => Err(code),
            _ => {
                let Some(chain) = self.gpu.current.as_ref() else {
                    self.ledger.return_unused(frame);
                    return Ok(Step::Wait);
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
        let index = match acquired {
            Ok((index, suboptimal)) => {
                self.ledger.acquired(index, chain_id);
                self.log(Event::Acquire { index });
                // Judged after the frame is presented (a failure of the judgement then finds the ledger whole).
                self.suboptimal_pending |= suboptimal;
                index
            }
            Err(code) => {
                self.log(Event::AcquireError(code));
                self.ledger.return_unused(frame);
                return match code {
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
                };
            }
        };
        if self.hooks.abandon_frames > 0 {
            self.hooks.abandon_frames -= 1;
            self.abandon(&frame, index)?;
            return Ok(Step::Wait);
        }
        // ---- record and submit ----
        let recorded = match self.gpu.current.as_ref() {
            Some(chain) => self.gpu.record(
                chain,
                index,
                rgb,
                prepared.draw.map(|draw| (draw.params, draw.upload)),
            ),
            None => Err((
                "vkBeginCommandBuffer",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            )),
        };
        if let Err(call_failure) = recorded {
            let fatal = failure(call_failure);
            self.abandon(&frame, index)?;
            return Err(fatal);
        }
        if prepared.draw.is_some() && self.hooks.abandon_after_record > 0 {
            // The upload is recorded and the frame is given back before its submit: the texture is not uploaded,
            // and its staging buffer is kept for the next frame.
            self.hooks.abandon_after_record -= 1;
            self.abandon(&frame, index)?;
            return Ok(Step::Wait);
        }
        let submitted = match TestHooks::next(&mut self.hooks.submit) {
            Some(code) => Err(code),
            None => self.gpu.submit(
                vk::Semaphore::from_raw(frame.acquire),
                vk::Semaphore::from_raw(frame.slot.semaphore),
                vk::Fence::from_raw(frame.submit_fence),
            ),
        };
        if let Err(code) = submitted {
            if code == vk::Result::ERROR_DEVICE_LOST {
                self.ledger.device_lost();
                return Err(failure(("vkQueueSubmit", code)));
            }
            let fatal = failure(("vkQueueSubmit", code));
            self.abandon(&frame, index)?;
            return Err(fatal);
        }
        let number = self.ledger.submitted(&frame);
        self.log(Event::Submit);
        self.quad_submitted(number, prepared);
        // ---- present ----
        self.present(&frame, index, rgb)
    }

    /// An image acquired and not submitted: released with `vkReleaseSwapchainImagesEXT`, and its acquire
    /// semaphore's pending signal consumed by an empty submit that waits on it.
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
        match self.gpu.drain(
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
        let result: Result<bool, vk::Result> = match (injected, self.gpu.current.as_ref()) {
            (_, None) => Err(vk::Result::ERROR_SURFACE_LOST_KHR),
            // A queued error: the real present runs first, so its semaphore wait and fence are queued, and
            // then the ledger sees the injected result.
            (
                Some(
                    code @ (vk::Result::SUBOPTIMAL_KHR
                    | vk::Result::ERROR_OUT_OF_DATE_KHR
                    | vk::Result::ERROR_SURFACE_LOST_KHR),
                ),
                Some(chain),
            ) => {
                let real = self.gpu.present(chain, index, semaphore, fence);
                match (real, code) {
                    (Err(real), _) if real == vk::Result::ERROR_DEVICE_LOST => Err(real),
                    (_, vk::Result::SUBOPTIMAL_KHR) => Ok(true),
                    (_, other) => Err(other),
                }
            }
            // Nothing queued: the real call is not made.
            (Some(code), Some(_)) => Err(code),
            (None, Some(chain)) => self.gpu.present(chain, index, semaphore, fence),
        };
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
                // Device lost or an error the ledger has no transition for: nothing can be proven, so
                // every object is kept.
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
                let queued = matches!(call, PresentCall::Success | PresentCall::Suboptimal);
                if queued {
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
                // An out-of-date frame was not displayed, and a rebuild that was asked for runs on the next step
                // without waiting for another trigger.
                Ok(Step::Drawn {
                    again: call == PresentCall::OutOfDate || self.rebuild_wanted,
                })
            }
            Presented::NeedsDrain => {
                // Nothing was queued; the render-finished semaphore still has a pending signal from the
                // submit, so it is drained by a submit that waits on it before it can be reused.
                match self
                    .gpu
                    .drain(semaphore, vk::Fence::from_raw(frame.spare_fence))
                {
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

    /// A frame was queued for presentation.
    fn drawn(&mut self, rgb: [f32; 3]) {
        self.presented += 1;
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

    // ---- teardown ----

    /// After the stop: proves every outstanding submit fence, present fence (the retired swap chain's
    /// included) and drain against the one absolute stop deadline, with bounded waits and never
    /// `vkDeviceWaitIdle`; destroys in the order of design §7 and sends `Proven` only if all were proven.
    /// Otherwise it destroys nothing.
    fn teardown(mut self) -> Outcome {
        if self.hooks.panic_at_stop {
            unwind::forced_panic(self.hooks.forced_panic_payload, "a forced stop-time panic");
        }
        let deadline = lock(&self.shared.slot)
            .stop_deadline
            .unwrap_or_else(Instant::now);
        loop {
            // A fence error here is a failed proof: reported, then judged by the ledger below.
            if let Err(Fatal(message)) = self.service_fences() {
                self.fail(message);
            }
            if self.ledger.proven() {
                self.publish();
                self.gpu.destroy_all();
                self.gpu.release_holders();
                return Outcome::Proven;
            }
            if self.ledger.is_lost() {
                self.publish();
                return Outcome::Unproven(format!(
                    "{}; every object, the instance and the loader were kept",
                    self.ledger.describe_outstanding()
                ));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.publish();
                return Outcome::Unproven(format!(
                    "the stop deadline passed with {}; every object, the instance and the loader were kept",
                    self.ledger.describe_outstanding()
                ));
            }
            let never = Self::flag(self.hooks.fences_never_signal.as_ref());
            let held = Self::flag(self.hooks.present_fences_held.as_ref());
            let fences: Vec<vk::Fence> = self
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
                match self.gpu.wait_any(&fences, timeout) {
                    Ok(()) | Err(vk::Result::TIMEOUT) => {}
                    Err(code) => {
                        if code == vk::Result::ERROR_DEVICE_LOST {
                            self.ledger.device_lost();
                        }
                        self.fail(format!("vkWaitForFences failed: {code:?}"));
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
    }
}
