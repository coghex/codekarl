//! The Vulkan presenter's handle, driven from the main thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ash::vk;

use crate::context::VulkanContext;
use crate::frame::VulkanFrameSlot;
use crate::gate::BindlessLimits;
use crate::hooks::{POLL, TestHooks};
use crate::render::{render_main, scene_main, texture_marker};
use crate::revisions::BindingNotice;
use crate::revisions::LedgerLimits;
use crate::scene::{SceneLimits, SceneSetup, TextureDriver};
use crate::shared::{
    CleanupResult, PresenterReport, STOP_BUDGET, SceneStats, Shared, Slot, Wake, lock,
};
use crate::slot::{QuadSkipped, VulkanQuadSlot};
use crate::textures::TextureLimits;
use crate::unwind;
use crate::{Error, Instance, Loader};

/// The Vulkan presenter's handle, driven from the main thread. Its methods never wait beyond a brief lock
/// to copy a small value. Dropping it sets the stop request and never waits; it does not join and releases
/// nothing the thread owns.
///
/// The handle holds its own clone of the [`Loader`] (and so does the render thread), so the library stays
/// loaded for as long as either can call it.
pub struct VulkanPresenter {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    ended: Option<CleanupResult>,
    _loader: Loader,
}

impl std::fmt::Debug for VulkanPresenter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VulkanPresenter")
    }
}

impl VulkanPresenter {
    /// Spawns the render thread for `surface` and returns at once. The thread picks a device that can
    /// present to the surface (with `VK_KHR_swapchain` and `VK_EXT_swapchain_maintenance1`), creates the
    /// device, the swap chain at `pixel_size` and the fixed pools, and draws the first frame; a failure on
    /// the thread comes through [`take_report`](Self::take_report) as `Failed`.
    ///
    /// `wake` is called on the render thread after each report and after its cleanup result. It must be
    /// short and must not block; a panic in it is caught and ignored.
    ///
    /// # Errors
    ///
    /// `Err` means no thread was spawned and no GPU object exists.
    ///
    /// # Safety
    ///
    /// `surface` must be a `VkSurfaceKHR` created on `instance` for a window that stays alive at least until
    /// the presenter's end is proven ([`poll_end`](Self::poll_end) returned [`CleanupResult::Proven`]) or
    /// the process ends, and nothing else may use the surface meanwhile. The instance must outlive the
    /// surface (it does: the presenter holds it until a proven end, and
    /// [`destroy_surface_and_instance`](crate::destroy_surface_and_instance) refuses while it is held).
    pub unsafe fn start(
        instance: &Instance,
        surface: vk::SurfaceKHR,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
    ) -> Result<Self, Error> {
        // SAFETY: the contract is this function's own.
        unsafe {
            Self::start_with_test_hooks(
                instance,
                surface,
                pixel_size,
                clear,
                wake,
                TestHooks::default(),
            )
        }
    }

    /// [`start`](Self::start) with a textured-quad slot: the render thread draws the slot's quad, if it holds one,
    /// over the clear in the same render pass, and reads the slot before its first frame (a scene published before
    /// the start is in the first frame). A publish to the slot marks a frame wanted and wakes the render thread
    /// only.
    ///
    /// Everything else is as for [`start`](Self::start), which stays as it was and draws the clear only. When the
    /// surface's format is not `B8G8R8A8_UNORM` or `R8G8B8A8_UNORM` the quad is not drawn: the window keeps
    /// clearing, and [`take_quad_notice`](Self::take_quad_notice) says why, once.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start): `Err` means no thread was spawned and no GPU object exists.
    ///
    /// # Safety
    ///
    /// The same contract as [`start`](Self::start).
    pub unsafe fn start_2d(
        instance: &Instance,
        surface: vk::SurfaceKHR,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        slot: Arc<VulkanQuadSlot>,
    ) -> Result<Self, Error> {
        let hooks = TestHooks {
            quad_slot: Some(slot),
            ..TestHooks::default()
        };
        // SAFETY: the contract is this function's own.
        unsafe { Self::start_with_test_hooks(instance, surface, pixel_size, clear, wake, hooks) }
    }

    /// The scene presenter: draws an ordinary instanced, batched scene through the revision ledger (milestone 5.5, issue 2a)
    /// on the shared `context`, which must have been created for `surface` (M5.5-1). The render thread takes the context's
    /// `RenderQueue`, and every submit and present goes through it. The swap chain, render pass, pools and present ledger
    /// are created on the context's device, with the rules of the crate documentation unchanged. `start` and `start_2d`
    /// keep their own device and their behaviour.
    ///
    /// The thread starts the texture store and ledger on the context with `texture_limits`, `ledger_limits` and
    /// `max_textures`, uploads the checker, allocates the instance buffer for `limits.max_quads` (refusing one that does
    /// not fit `limits.binding_geometry_bytes` as a `Failed` report naming both numbers), and runs `driver` on its own
    /// thread's passes. The renderer publishes frames to `frames`; a published frame wants a frame. The legacy quad of a
    /// frame is the compatibility lane's (a later part) and is not drawn by this entry yet.
    ///
    /// # Errors
    ///
    /// `Err` means no thread was spawned and no GPU object was made by this call.
    ///
    /// # Safety
    ///
    /// The same contract as [`start`](Self::start): `surface` must be a `VkSurfaceKHR` created on the context's instance for
    /// a window that stays alive at least until the presenter's end is proven, nothing else may use the surface meanwhile,
    /// and the context must have been created for this surface.
    #[expect(
        clippy::too_many_arguments,
        reason = "start_2d's arguments plus the context, the limits and the driver the specification names"
    )]
    pub unsafe fn start_scene<K, D>(
        surface: vk::SurfaceKHR,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        frames: Arc<VulkanFrameSlot<K>>,
        context: Arc<VulkanContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        max_textures: usize,
        bindless: BindlessLimits,
        limits: SceneLimits,
        driver: D,
    ) -> Result<Self, Error>
    where
        K: Copy + Eq + std::hash::Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        let setup = SceneSetup::new(
            context,
            texture_limits,
            ledger_limits,
            max_textures,
            bindless,
            limits,
            driver,
            frames,
            None,
        );
        // SAFETY: the contract is this function's own.
        unsafe {
            Self::start_scene_with_test_hooks(
                surface,
                pixel_size,
                clear,
                wake,
                &setup,
                TestHooks::default(),
            )
        }
    }

    /// [`start_scene`](Self::start_scene) with faults injected by the tests. Not part of the interface.
    ///
    /// # Errors
    ///
    /// As [`start_scene`](Self::start_scene), and a forced `fail_start`.
    ///
    /// # Safety
    ///
    /// The same contract as [`start_scene`](Self::start_scene). `doc(hidden)` does not make the surface safe.
    #[doc(hidden)]
    pub unsafe fn start_scene_with_test_hooks(
        surface: vk::SurfaceKHR,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        setup: &SceneSetup,
        hooks: TestHooks,
    ) -> Result<Self, Error> {
        if hooks.fail_start {
            return Err(Error(
                "a forced failure before the thread was spawned".to_owned(),
            ));
        }
        if surface == vk::SurfaceKHR::null() {
            return Err(Error("the surface is null".to_owned()));
        }
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                clear,
                size: pixel_size,
                dirty: true,
                epoch: 0,
                texture_work: false,
                stop: false,
                stop_deadline: None,
            }),
            signal: Condvar::new(),
            reports: Mutex::new(VecDeque::new()),
            result: Mutex::new(None),
            wake,
            stop_budget: hooks.stop_budget.unwrap_or(STOP_BUDGET),
            drawn_generation: AtomicU64::new(0),
            drawn_any: AtomicBool::new(false),
            notice: Mutex::new(None),
            binding_notice: Mutex::new(None),
            scene_stats: Mutex::new(SceneStats::default()),
        });
        // A published frame wants a frame, and only the render thread is woken. The wake holds the presenter's state
        // weakly. A frame published before this point is read by the first frame, which the slot's initial `dirty`
        // guarantees.
        let weak = Arc::downgrade(&shared);
        setup.0.register_wake(Arc::new(move || {
            if let Some(shared) = weak.upgrade() {
                let mut slot = lock(&shared.slot);
                slot.dirty = true;
                slot.epoch += 1;
                drop(slot);
                shared.signal.notify_all();
            }
        }));
        let loader = setup.0.context().holders().1;
        let thread_shared = Arc::clone(&shared);
        let thread_setup = setup.clone();
        let spawned = thread::Builder::new()
            .name("ck-vulkan-render".to_owned())
            .spawn(move || scene_main(&thread_shared, thread_setup, surface, hooks));
        match spawned {
            Ok(thread) => Ok(Self {
                shared,
                thread: Some(thread),
                ended: None,
                _loader: loader,
            }),
            Err(error) => {
                setup.0.clear_wake();
                Err(Error(format!(
                    "the render thread could not be spawned: {error}"
                )))
            }
        }
    }

    /// A handle that marks "texture work pending" and wakes the render thread, for the driver's owner to give its own
    /// producers (the resource service's upload and retirement wakes). It never blocks and holds the presenter weakly: it
    /// does nothing after the presenter ended.
    #[must_use]
    pub fn texture_wake(&self) -> Wake {
        texture_marker(&self.shared)
    }

    /// The scene presenter's counters (a copy; a presenter started without a scene reports zeros).
    #[must_use]
    pub fn scene_stats(&self) -> SceneStats {
        *lock(&self.shared.scene_stats)
    }

    /// [`start`](Self::start) with faults injected by the tests. Not part of the interface.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start), and a forced `fail_start`.
    ///
    /// # Safety
    ///
    /// The same contract as [`start`](Self::start). `doc(hidden)` does not make the surface safe.
    #[doc(hidden)]
    pub unsafe fn start_with_test_hooks(
        instance: &Instance,
        surface: vk::SurfaceKHR,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        hooks: TestHooks,
    ) -> Result<Self, Error> {
        if hooks.fail_start {
            return Err(Error(
                "a forced failure before the thread was spawned".to_owned(),
            ));
        }
        if surface == vk::SurfaceKHR::null() {
            return Err(Error("the surface is null".to_owned()));
        }
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                clear,
                size: pixel_size,
                dirty: true,
                epoch: 0,
                texture_work: false,
                stop: false,
                stop_deadline: None,
            }),
            signal: Condvar::new(),
            reports: Mutex::new(VecDeque::new()),
            result: Mutex::new(None),
            wake,
            stop_budget: hooks.stop_budget.unwrap_or(STOP_BUDGET),
            drawn_generation: AtomicU64::new(0),
            drawn_any: AtomicBool::new(false),
            notice: Mutex::new(None),
            binding_notice: Mutex::new(None),
            scene_stats: Mutex::new(SceneStats::default()),
        });
        if let Some(quads) = &hooks.quad_slot {
            // A change in the slot wants a frame, and only the render thread is woken. The wake holds the
            // presenter's state weakly, so a slot that outlives the presenter keeps nothing of it alive. A value
            // published before this point is read by the first frame, which the slot's initial `dirty` guarantees.
            let weak = Arc::downgrade(&shared);
            quads.register_wake(Arc::new(move || {
                if let Some(shared) = weak.upgrade() {
                    let mut slot = lock(&shared.slot);
                    slot.dirty = true;
                    slot.epoch += 1;
                    drop(slot);
                    shared.signal.notify_all();
                }
            }));
        }
        let quads = hooks.quad_slot.clone();
        let loader = instance.inner.loader.clone();
        let thread_loader = loader.clone();
        let inner = Arc::clone(&instance.inner);
        let thread_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("ck-vulkan-render".to_owned())
            .spawn(move || render_main(&thread_shared, inner, thread_loader, surface, hooks));
        match spawned {
            Ok(thread) => Ok(Self {
                shared,
                thread: Some(thread),
                ended: None,
                _loader: loader,
            }),
            Err(error) => {
                if let Some(quads) = quads {
                    quads.clear_wake();
                }
                Err(Error(format!(
                    "the render thread could not be spawned: {error}"
                )))
            }
        }
    }

    /// Sets the clear colour (components 0..=1). A different colour gives one new frame.
    pub fn set_clear(&self, rgb: [f32; 3]) {
        let mut slot = lock(&self.shared.slot);
        // Bit-for-bit: a colour set again to the very same value is no change.
        if slot.clear.map(f32::to_bits) != rgb.map(f32::to_bits) {
            slot.clear = rgb;
            slot.dirty = true;
            slot.epoch += 1;
            drop(slot);
            self.shared.signal.notify_all();
        }
    }

    /// Sets the window's size in pixels. A different size gives one new frame; a zero dimension suspends
    /// rendering (no zero-size swap chain is created) until a non-zero size follows.
    pub fn set_pixel_size(&self, width: u32, height: u32) {
        let mut slot = lock(&self.shared.slot);
        if slot.size != (width, height) {
            slot.size = (width, height);
            slot.dirty = true;
            slot.epoch += 1;
            drop(slot);
            self.shared.signal.notify_all();
        }
    }

    /// Asks for a frame.
    pub fn request_redraw(&self) {
        let mut slot = lock(&self.shared.slot);
        slot.dirty = true;
        slot.epoch += 1;
        drop(slot);
        self.shared.signal.notify_all();
    }

    /// The next report, if any.
    #[must_use]
    pub fn take_report(&self) -> Option<PresenterReport> {
        lock(&self.shared.reports).pop_front()
    }

    /// The format notice, if the render thread recorded one and it was not taken yet: at most once per run, and
    /// never a report. It says the quad is not drawn because the surface's format is not one textured drawing
    /// supports (the window keeps clearing). The render thread calls the presenter's `wake` after recording it.
    #[must_use]
    pub fn take_quad_notice(&self) -> Option<QuadSkipped> {
        lock(&self.shared.notice).take()
    }

    /// The scene's binding notice, if the render thread recorded one and it was not taken yet: at most once per run, and never
    /// a report. It says how the textures are bound: through the bindless tables (with the support query's answer), or, when
    /// the device gate refused (the text names every failed row), through per-revision descriptor sets (the owner's logged
    /// fallback). The render thread calls the presenter's `wake` after recording it.
    #[must_use]
    pub fn take_binding_notice(&self) -> Option<BindingNotice> {
        lock(&self.shared.binding_notice).take()
    }

    /// The generation of the scene the last frame queued for presentation drew, with or without a quad; `None`
    /// before the first such frame (and always for a presenter started without a slot). Read without waiting: a
    /// bounded latest observation, not a history and not visual proof.
    #[must_use]
    pub fn drawn_generation(&self) -> Option<u64> {
        // The render thread writes the generation first and the flag second, so a set flag has a generation.
        self.shared
            .drawn_any
            .load(Ordering::Acquire)
            .then(|| self.shared.drawn_generation.load(Ordering::Acquire))
    }

    /// Asks the thread to stop. The first call records the one **absolute stop deadline** (now plus 1.5 s)
    /// that the thread's teardown, and an acquire that was blocking when the stop arrived, share; a repeated
    /// call does not move it.
    pub fn request_stop(&self) {
        let mut slot = lock(&self.shared.slot);
        if slot.stop_deadline.is_none() {
            slot.stop_deadline = Some(Instant::now() + self.shared.stop_budget);
        }
        slot.stop = true;
        drop(slot);
        self.shared.signal.notify_all();
    }

    /// Waits at most `within` for the render thread to end. `Some` only once the thread has been observed
    /// finished (`is_finished`, then joined); the result is [`CleanupResult::Proven`] only if the thread
    /// sent an explicit successful result before ending.
    pub fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        if let Some(result) = &self.ended {
            return Some(result.clone());
        }
        let give_up = Instant::now().checked_add(within);
        loop {
            if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
                if let Some(thread) = self.thread.take() {
                    // A thread that unwound past its own boundary: its payload is disposed of without unwinding.
                    if let Err(payload) = thread.join() {
                        unwind::dispose(payload);
                    }
                }
                let result =
                    lock(&self.shared.result)
                        .take()
                        .unwrap_or_else(|| CleanupResult::Unproven {
                            reason: "the render thread ended without a cleanup result".to_owned(),
                        });
                self.ended = Some(result.clone());
                return Some(result);
            }
            let left = give_up.map_or(POLL, |limit| {
                limit.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return None;
            }
            thread::sleep(left.min(Duration::from_millis(1)));
        }
    }
}

impl Drop for VulkanPresenter {
    fn drop(&mut self) {
        // The same stop operation, deadline and all; it never waits, never joins and releases nothing.
        self.request_stop();
    }
}
