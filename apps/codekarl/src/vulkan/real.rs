//! The real stack of the `glfw-vulkan` path: `ck-window-glfw`'s window and `ck-gpu-vulkan`'s loader,
//! instance, surface and presenter (builds with the `vulkan` feature).
//!
//! **The five `unsafe` operations of the Vulkan integration**, each in exactly one place below with its
//! `SAFETY` comment against its contract: the `vkGetInstanceProcAddr` pointer conversion and
//! `VulkanLoaderForGlfw::new` ([`Real::glfw`]), `GlfwHandles::create_surface` ([`Real::create_surface`]), `VulkanPresenter::start_2d` ([`Real::start`]) and
//! `destroy_surface_and_instance` ([`Real::destroy`]). A source check in this module's tests counts them.

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Duration;

use ck_gpu_vulkan::vk::Handle as _;
use ck_gpu_vulkan::{
    CleanupResult as GpuCleanup, Instance, Loader, PresenterReport as GpuReport, VulkanPresenter,
    VulkanQuadSlot, create_instance, destroy_surface_and_instance, load_loader, vk,
};
use ck_render_2d::{QuadSink, VulkanSinkSide};
use ck_window::{CleanupResult, PixelSize, PresenterReport};
use ck_window_glfw::{GlfwBackend, GlfwHandles, VulkanLoaderForGlfw};

use super::{Ended, QuadSide, Refused, Running, Stack};

/// The real stack.
pub(super) struct Real;

impl Stack for Real {
    type Handles = GlfwHandles;
    type Glfw = GlfwBackend;
    type Loader = Loader;
    type Instance = Instance;
    type Surface = vk::SurfaceKHR;
    type Started = Started;
    type Slot = Arc<VulkanQuadSlot>;
    type Side = VulkanSinkSide;

    fn load_loader(&self) -> Result<Loader, String> {
        // By explicit absolute path (`$VULKAN_SDK/lib`, `/usr/local/lib`, `/opt/homebrew/lib`, then the
        // platform's default search): no dynamic-loader variable is needed.
        load_loader().map_err(|error| error.to_string())
    }

    fn glfw(&self, loader: &Loader) -> Result<GlfwBackend, String> {
        let proc_addr = loader.get_instance_proc_addr();
        let keepalive = Box::new(loader.clone());
        // SAFETY: both types are the C-ABI `vkGetInstanceProcAddr` signature (a pointer-sized instance
        // handle and a C string in, a function pointer out), so the conversion keeps the callee. The
        // pointer is valid only while a clone of `loader` lives, which the next operation's keep-alive
        // guarantees.
        let proc_addr = unsafe {
            std::mem::transmute::<
                ck_gpu_vulkan::GetInstanceProcAddr,
                ck_window_glfw::GetInstanceProcAddr,
            >(proc_addr)
        };
        // SAFETY: `proc_addr` is this loader's own `vkGetInstanceProcAddr` (converted above), and
        // `keepalive` is a clone of that very loader, which keeps the library loaded for as long as the
        // value lives (the backend holds it until after `glfwTerminate` and its NULL reset), as the
        // constructor's contract asks.
        let value = unsafe { VulkanLoaderForGlfw::new(proc_addr, keepalive) };
        GlfwBackend::new(Some(value))
    }

    fn create_instance(&self, loader: &Loader, handles: &GlfwHandles) -> Result<Instance, String> {
        create_instance(loader, handles.required_instance_extensions())
            .map_err(|error| error.to_string())
    }

    fn create_surface(
        &self,
        handles: &GlfwHandles,
        instance: &Instance,
    ) -> Result<vk::SurfaceKHR, String> {
        let raw = usize::try_from(instance.raw_handle())
            .map_err(|_| "the instance handle does not fit a pointer".to_owned())?;
        // SAFETY: `raw` is the live `VkInstance` of `instance`, which the caller holds for the call; this
        // runs on the main thread (the loop's thread) and the window behind `handles` is alive (it is
        // destroyed only by the loop's teardown, after the surface).
        let surface =
            unsafe { handles.create_surface(std::ptr::without_provenance_mut::<c_void>(raw)) }?;
        Ok(vk::SurfaceKHR::from_raw(surface))
    }

    fn quad_side(&self, wake: Arc<dyn Fn() + Send + Sync>) -> VulkanSinkSide {
        VulkanSinkSide::new(wake)
    }

    fn start(
        &self,
        instance: &Instance,
        surface: vk::SurfaceKHR,
        size: PixelSize,
        clear: [f32; 3],
        wake: Arc<dyn Fn() + Send + Sync>,
        slot: Arc<VulkanQuadSlot>,
    ) -> Result<Started, String> {
        // SAFETY: `surface` was created on `instance` for the window `ck_window::run` holds, which outlives
        // the presenter's proven end (the loop destroys the window only after a `Proven` end, and never
        // otherwise); nothing else uses the surface meanwhile, and the surface and the instance are
        // destroyed only after that end (`Real::destroy`), in that order. `slot` holds no Vulkan object.
        let presenter = unsafe {
            VulkanPresenter::start_2d(
                instance,
                surface,
                (size.width, size.height),
                clear,
                wake,
                slot,
            )
        }
        .map_err(|error| error.to_string())?;
        Ok(Started(presenter))
    }

    fn destroy(
        &self,
        instance: Instance,
        surface: Option<vk::SurfaceKHR>,
        ended: Ended,
    ) -> Result<(), Refused<Instance>> {
        let proof = match ended {
            Ended::Proven => Some(GpuCleanup::Proven),
            Ended::NoPresenter => None,
        };
        // SAFETY: `surface` (or null) was created on `instance`; nothing uses it any more (the presenter
        // ended `Proven`, or none was ever started), and the window it belongs to is alive: the loop
        // destroys the window after this call.
        unsafe {
            destroy_surface_and_instance(
                instance,
                surface.unwrap_or_else(vk::SurfaceKHR::null),
                proof.as_ref(),
            )
        }
        .map_err(|refused| Refused {
            surface_destroyed: refused.surface_destroyed,
            cause: refused.error.to_string(),
            instance: refused.instance,
        })
    }
}

impl QuadSide for VulkanSinkSide {
    type Slot = Arc<VulkanQuadSlot>;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        VulkanSinkSide::new_sink(self)
    }

    fn accept(&self, sink: &Arc<dyn QuadSink>) -> Result<Arc<VulkanQuadSlot>, String> {
        VulkanSinkSide::accept(self, sink)
    }

    fn take_report(&self) -> Option<PresenterReport> {
        VulkanSinkSide::take_report(self)
    }
}

/// The real presenter handle, as the loop's [`Running`].
pub(super) struct Started(VulkanPresenter);

impl Running for Started {
    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.0.set_clear(rgb);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.0.set_pixel_size(width, height);
    }

    fn request_redraw(&mut self) {
        self.0.request_redraw();
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.0.take_report().map(|report| match report {
            GpuReport::FirstFrameQueued => PresenterReport::FirstFrameQueued,
            GpuReport::Failed { message } => PresenterReport::Failed { message },
        })
    }

    fn request_stop(&mut self) {
        self.0.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.0.poll_end(within).map(|result| match result {
            GpuCleanup::Proven => CleanupResult::Proven,
            GpuCleanup::Unproven { reason } => CleanupResult::Unproven { reason },
        })
    }

    fn drawn_generation(&self) -> Option<u64> {
        self.0.drawn_generation()
    }

    fn take_notice(&mut self) -> Option<String> {
        self.0
            .take_quad_notice()
            .map(|notice| format!("{:?}", notice.format))
    }
}
