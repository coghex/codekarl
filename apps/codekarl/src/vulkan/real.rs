//! The real stack of the `glfw-vulkan` path: `ck-window-glfw`'s window and `ck-gpu-vulkan`'s loader,
//! instance, surface, device context and scene presenter (builds with the `vulkan` feature).
//!
//! **The five `unsafe` operations of the Vulkan integration**, each in exactly one place below with its
//! `SAFETY` comment against its contract: the `vkGetInstanceProcAddr` pointer conversion and
//! `VulkanLoaderForGlfw::new` ([`Real::glfw`]), `GlfwHandles::create_surface` ([`Real::create_surface`]), `VulkanPresenter::start_scene` ([`Real::start`]) and
//! `destroy_surface_and_instance` ([`Real::destroy`]). A source check in this module's tests counts them.
//!
//! **The device context** is made in [`Real::start`], on the main thread, after the window and the surface exist
//! (`VulkanContext::new(&instance, Some(surface))`): the device must present to the window's surface, so unlike the Metal
//! path it cannot be made before any window. It moves into `start_scene`; no clone of it is kept, because the destroy helper
//! refuses while anything still holds the instance.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ck_gpu_vulkan::vk::Handle as _;
use ck_gpu_vulkan::{
    BindlessLimits, CleanupResult as GpuCleanup, Instance, LedgerLimits, Loader,
    PresenterReport as GpuReport, SceneLimits, TextureLimits, ValidationLogHandle, VulkanContext,
    VulkanFrameSlot, VulkanPresenter, create_instance_draining, destroy_surface_and_instance,
    load_loader, vk,
};
use ck_render_2d::{QuadSink, VulkanResidentDriver, VulkanResourceLimits, VulkanSinkSide};
use ck_resources::TextureKey;
use ck_window::{CleanupResult, PixelSize, PresenterReport};
use ck_window_glfw::{GlfwBackend, GlfwHandles, VulkanLoaderForGlfw};

use super::{
    BindingNote, Ended, QuadSide, Refused, Resources, Running, Stack, ValidationDrain,
    ValidationEntry,
};
use crate::windowed::VulkanFigures;

/// The real stack. It keeps the log of a failed instance creation (which returns no instance, hence no handle) until the run
/// takes it.
#[derive(Default)]
pub(super) struct Real {
    creation_log: Mutex<ValidationDrain>,
}

impl Stack for Real {
    type Handles = GlfwHandles;
    type Glfw = GlfwBackend;
    type Loader = Loader;
    type Instance = Instance;
    type Surface = vk::SurfaceKHR;
    type Started = Started;
    type Slot = Arc<VulkanFrameSlot<TextureKey>>;
    type Driver = VulkanResidentDriver;
    type Side = VulkanSinkSide;
    type Log = ValidationLogHandle;

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
        create_instance_draining(loader, handles.required_instance_extensions()).map_err(
            |failure| {
                // The messages a failed or partial creation recorded are kept, every one, for the run to write.
                *self
                    .creation_log
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = ValidationDrain {
                    entries: failure
                        .log
                        .entries
                        .into_iter()
                        .map(|entry| ValidationEntry {
                            text: entry.text,
                            tolerated: entry.tolerated,
                            loader_notice: entry.loader_notice,
                        })
                        .collect(),
                    dropped: failure.log.dropped,
                };
                failure.error.to_string()
            },
        )
    }

    fn take_creation_log(&self) -> ValidationDrain {
        std::mem::take(
            &mut *self
                .creation_log
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    fn validation_log(&self, instance: &Instance) -> ValidationLogHandle {
        instance.validation_log()
    }

    fn drain_validation(&self, log: &ValidationLogHandle) -> ValidationDrain {
        let drained = log.drain();
        ValidationDrain {
            entries: drained
                .entries
                .into_iter()
                .map(|entry| ValidationEntry {
                    text: entry.text,
                    tolerated: entry.tolerated,
                    loader_notice: entry.loader_notice,
                })
                .collect(),
            dropped: drained.dropped,
        }
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

    fn quad_side(
        &self,
        wake: Arc<dyn Fn() + Send + Sync>,
        resources: &Resources,
    ) -> VulkanSinkSide {
        // Only the adoption cap is the side's; the store, ledger and scene limits are `start`'s.
        VulkanSinkSide::with_resources(
            wake,
            resources.service.clone(),
            resources.relay.clone(),
            VulkanResourceLimits {
                max_adoptions_per_frame: resources.config.max_adoptions_per_frame,
            },
        )
    }

    fn start(
        &self,
        instance: &Instance,
        surface: vk::SurfaceKHR,
        size: PixelSize,
        clear: [f32; 3],
        wake: Arc<dyn Fn() + Send + Sync>,
        slot: Arc<VulkanFrameSlot<TextureKey>>,
        driver: VulkanResidentDriver,
        resources: &Resources,
    ) -> Result<Started, String> {
        // The device context, made for this surface: its failure is a presenter start failure like any other.
        let context =
            VulkanContext::new(instance, Some(surface)).map_err(|error| error.to_string())?;
        let config = &resources.config;
        let max_textures = usize::try_from(config.service.max_textures)
            .map_err(|_| "[resources] max_textures does not fit a pointer".to_owned())?;
        // SAFETY: `surface` was made on `instance` for the window `ck_window::run` holds until the presenter's proven end;
        // `context` was made for it just above and moves into the presenter (no clone kept); nothing else uses the surface
        // meanwhile, and `Real::destroy` destroys it and the instance only after that end.
        let presenter = unsafe {
            VulkanPresenter::start_scene(
                surface,
                (size.width, size.height),
                clear,
                wake,
                slot,
                context,
                TextureLimits {
                    max_prepared_images: config.max_prepared_images,
                    max_upload_images_per_batch: config.max_upload_images_per_batch,
                    max_upload_bytes_per_batch: config.max_upload_bytes_per_batch,
                    staging_bytes: config.staging_bytes,
                    texture_bytes: config.texture_bytes,
                },
                LedgerLimits {
                    max_extra_revisions: config.max_extra_revisions,
                    max_adoptions_per_frame: config.max_adoptions_per_frame,
                },
                max_textures,
                BindlessLimits {
                    texture_slots: resources.bindless_texture_slots,
                },
                SceneLimits {
                    max_quads: resources.max_quads,
                    binding_geometry_bytes: config.binding_geometry_bytes,
                },
                driver,
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
    type Slot = Arc<VulkanFrameSlot<TextureKey>>;
    type Driver = VulkanResidentDriver;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        VulkanSinkSide::new_sink(self)
    }

    fn accept_frames(
        &self,
        sink: &Arc<dyn QuadSink>,
    ) -> Result<Arc<VulkanFrameSlot<TextureKey>>, String> {
        VulkanSinkSide::accept_frames(self, sink)
    }

    fn resident_driver(&self) -> Result<VulkanResidentDriver, String> {
        VulkanSinkSide::resident_driver(self)
    }

    fn connect(&self, texture_wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        VulkanSinkSide::connect(self, texture_wake)
    }

    fn disconnect(&self) -> Result<(), String> {
        VulkanSinkSide::disconnect(self)
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

    fn take_binding_notice(&mut self) -> Option<BindingNote> {
        self.0.take_binding_notice().map(|notice| BindingNote {
            fallback: notice.fallback,
            text: notice.text,
        })
    }

    fn texture_wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.0.texture_wake()
    }

    fn figures(&self) -> Option<VulkanFigures> {
        let stats = self.0.scene_stats();
        Some(VulkanFigures {
            textures_created: stats.textures_created,
            texture_bytes_peak: stats.texture_bytes_peak,
            staging_bytes_peak: stats.staging_bytes_peak,
            checker_bytes: stats.checker_bytes,
            frames: stats.frames,
            bindless_slots: stats.bindless_slots,
        })
    }
}
