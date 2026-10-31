//! The native macOS path (`native-metal`): the `AppKit` backend of `ck-window-cocoa` and the Metal
//! presenter of `ck-gpu-metal`, joined for `ck-window`'s loop (design §3, §4.1).
//!
//! The run closure gives `ck-window`'s loop a [`Renderer`] of `ck-render-2d`'s [`MetalBackend`] built on the run's
//! resource service (`MetalBackend::with_resources`, milestone 5-8c): the service and the wake relay 8a composed, a
//! [`MetalContext`] made first, on the main thread, and the backend limits parsed from `[resources]` and `[render]`. The
//! backend passes the window's `CAMetalLayer` to the scene presenter. This module makes the one `unsafe` call of the
//! path, under that constructor's documented contract (the driver keeps the layer alive until the presenter's end is
//! proven, as `ck_window::run` does); its `wake` is the notifier's `wake`, so a sleeping loop is woken by the render
//! thread's reports and by a refused quad. The legacy `render.quad` goes through the compatibility lane on this path
//! (the composition's upgraded flag), so CK-119's first-frame gate applies to it.
//!
//! **The Metal device context is made before any window** (`run_closure_with`'s factory; production uses
//! `MetalContext::new`): a Mac without a device is found here, the run opens no window, posts no `Opened` and fails with
//! the same `cannot start the presenter: ...` text a presenter start failure gives, instead of after the window opened.
//!
//! This module only exists in builds with the `metal` feature on macOS.

use std::rc::Rc;
use std::sync::Arc;

use std::time::Duration;

use ck_gpu_metal::{BindlessLimits, LedgerLimits, MetalContext, TextureLimits};
use ck_log::Level;
use ck_render_2d::{Gpu2d, MetalBackend, QuadSink, Renderer, ResourceBackendLimits};
use ck_runtime::{RunContext, RunEnd};
use ck_window::{CleanupResult, PixelSize, PresenterReport, WindowDesc};
use ck_window_cocoa::{CocoaBackend, CocoaHandles};

use crate::lifecycle::RESOURCES;
use crate::metrics::{MetricsHub, SUMMARY_WAIT};
use crate::metrics_record::{Figures, summary};
use crate::windowed::{self, GpuFigures, RunClosure, RunEnv, RunNotes};

/// [`MetalBackend`] that says, at `debug` level, the latest committed scene generation when the first frame is
/// reported: once, when the loop first sees the first-frame report and the backend has a drawn generation to
/// read (the accepted [`MetalBackend::drawn_generation`], which the render thread stores before it reports the
/// frame). It is the **latest** committed generation, not a history of the first frame: a scene change or a
/// metrics change before the loop looks would show its own generation. It claims nothing about what is on the
/// screen. A plain record of the real run, for whoever reads the log, and what the real-window test looks for
/// (with a fixture that writes nothing after the window opens). Everything else is a forward.
///
/// The record never waits: `take_report` is `ck-window`'s non-waiting call, so it goes through `try_log` and a
/// refused record is dropped and counted by the logger.
struct Reporting {
    inner: MetalBackend,
    /// The first frame was reported, and its record has not been written yet.
    pending: bool,
    /// Where the presenter's last numbers go when its end is observed.
    notes: Rc<RunNotes>,
    /// The run's measurement window (`diagnostics.metrics_mark`); the one `metrics` record is written from it.
    metrics: Option<Arc<MetricsHub>>,
    /// The `metrics` record was written.
    reported: bool,
    /// The run's resource service, whose loader counters the record reads.
    resources: ck_resources::ResourceService,
}

/// The presenter's own spelling of a window the hub defined.
fn gpu_window(window: ck_window::MetricsWindow) -> ck_gpu_metal::MetricsWindow {
    ck_gpu_metal::MetricsWindow {
        id: window.id,
        start: window.start,
        end: window.end,
    }
}

impl Reporting {
    /// Hands the marked window to the presenter, once it is started: this thread owns the presenter, so the UI VM's mark only files
    /// the window in the hub and this runs at the next pass.
    fn forward_metrics(&mut self) {
        let Some(hub) = &self.metrics else { return };
        let Some(window) = hub.take_forward() else {
            return;
        };
        match self.inner.request_metrics(gpu_window(window)) {
            None => hub.unforwarded(),
            Some(Ok(())) => {}
            Some(Err(error)) => hub.presenter_refused(format!("{error:?}")),
        }
    }

    /// The run's one `metrics` record, written when the presenter's end is observed and its backend is still alive.
    fn write_metrics_record(&mut self) {
        let Some(hub) = self.metrics.clone() else {
            return;
        };
        self.reported = true;
        let Some(totals) = self.inner.metrics_totals() else {
            return;
        };
        let report = hub.report();
        let presenter_window = report
            .window
            .and_then(|window| self.inner.read_metrics(window.id, SUMMARY_WAIT));
        let loop_window = report
            .window
            .filter(|_| report.loop_attached)
            .and_then(|window| hub.loop_metrics().read(window.id, SUMMARY_WAIT));
        let figures = Figures {
            totals,
            scene: self.inner.scene_stats().unwrap_or_default(),
            loaders: self.resources.stats().loaders,
            timing_lost: presenter_window
                .as_ref()
                .map_or(0, |reading| reading.timing_lost),
            loop_totals: hub.loop_metrics().totals(),
            hub: report,
            presenter_window,
            loop_window,
        };
        crate::lifecycle::emit(
            Level::Debug,
            "metrics",
            format_args!("{}", summary(&figures)),
        );
    }
}

impl Gpu2d for Reporting {
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
        self.forward_metrics();
        let report = self.inner.take_report();
        if matches!(report, Some(PresenterReport::FirstFrameQueued)) {
            self.pending = true;
        }
        if self.pending
            && let Some(generation) = self.inner.drawn_generation()
        {
            self.pending = false;
            let _ = ck_log::try_log!(
                Level::Debug,
                "render",
                "first frame drawn, generation {generation}"
            );
        }
        report
    }

    fn request_stop(&mut self) {
        // The window loop has begun its teardown: this is the run's one end timestamp for the measured window, sent to both owners.
        if let Some(hub) = self.metrics.clone() {
            // A window marked and not yet handed over goes first, so the presenter holds it when the end arrives.
            self.forward_metrics();
            // The timestamp is read under the presenter's admission lock (and the loop's, inside `finish`).
            let mut guard = self.inner.lock_metrics_end();
            hub.finish(&mut |id, end| match guard.as_mut() {
                Some(guard) => guard.end(id, end).map_err(|error| format!("{error:?}")),
                None => Err("the presenter was not started".to_owned()),
            });
        }
        self.inner.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        let end = self.inner.poll_end(within);
        if end.is_some() && !self.reported {
            self.write_metrics_record();
        }
        if end.is_some() {
            // The presenter ended and its backend is still alive (the loop destroys it after this poll): the last
            // snapshot is copied to the run's notes for the final resource record, which is written after the
            // resource service reconciles, when the backend is gone.
            if let Some(stats) = self.inner.scene_stats() {
                *self.notes.gpu.borrow_mut() = Some(GpuFigures {
                    textures_created: stats.textures_created,
                    bindless_slots: stats.bindless_slots,
                    upload_texture_bytes_peak: stats.upload_texture_bytes_peak,
                    upload_staging_bytes_peak: stats.upload_staging_bytes_peak,
                    checker_texture_bytes: stats.checker_texture_bytes,
                    checker_staging_bytes: stats.checker_staging_bytes,
                    instance_buffer_bytes: stats.instance_buffer_allocated,
                    checker_instances_peak: stats.peak.checker,
                    frames: stats.frames,
                });
            }
        }
        end
    }
}

/// The Metal device context of a run: production makes it with `MetalContext::new` (a missing device is the error).
fn production_context() -> Result<Arc<MetalContext>, String> {
    MetalContext::new().map_err(|error| error.to_string())
}

/// The backend limits the run's `[resources]` and `[render] max_quads` parse to.
pub(crate) fn backend_limits(
    config: &crate::config::ResourceConfig,
    max_quads: usize,
    bindless_texture_slots: u32,
) -> ResourceBackendLimits {
    ResourceBackendLimits {
        texture: TextureLimits {
            max_prepared_images: config.max_prepared_images,
            max_upload_images_per_batch: config.max_upload_images_per_batch,
            max_upload_bytes_per_batch: config.max_upload_bytes_per_batch,
            staging_bytes: config.staging_bytes,
            texture_bytes: config.texture_bytes,
        },
        ledger: LedgerLimits {
            max_extra_revisions: config.max_extra_revisions,
            max_adoptions_per_frame: config.max_adoptions_per_frame,
        },
        max_quads,
        binding_geometry_bytes: config.binding_geometry_bytes,
        // The native path always draws through the bindless table: its slot count is the setting, covering the logical
        // textures and the extra revisions the configuration already checked (`BindlessLimits::validate` repeats the rule).
        bindless: BindlessLimits {
            texture_slots: bindless_texture_slots,
            max_textures: config.service.max_textures,
            max_extra_revisions: u32::try_from(config.max_extra_revisions).unwrap_or(u32::MAX),
        },
    }
}

/// The run closure of a windowed `native-metal` run: `ck-window`'s loop over the Cocoa backend and a
/// [`Renderer`] of `ck-render-2d`'s Metal backend on the run's resource service, attached to the run's scene bridge.
pub(crate) fn run_closure(env: RunEnv) -> RunClosure {
    run_closure_with(env, production_context)
}

/// [`run_closure`] with the Metal device context's factory given: the one seam the context's failure path is tested
/// through (production passes `MetalContext::new`; it is not a setting or a switch).
pub(crate) fn run_closure_with(
    env: RunEnv,
    context: impl FnOnce() -> Result<Arc<MetalContext>, String> + 'static,
) -> RunClosure {
    Box::new(move |_: &RunContext| -> RunEnd {
        // The device context first, before any window or native object: a failure is found here.
        let context = match context() {
            Ok(context) => context,
            Err(error) => {
                return windowed::fail_before_window(
                    &env,
                    format!("cannot start the presenter: {error}"),
                );
            }
        };
        let Some(service) = env.resources.clone() else {
            return windowed::fail_before_window(
                &env,
                "cannot start the presenter: the native path has no resource service".to_owned(),
            );
        };
        let limits = backend_limits(
            &env.resource_config,
            env.max_quads,
            env.bindless_texture_slots,
        );
        let notifier = Arc::clone(&env.notifier);
        let service_for_metrics = service.clone();
        // SAFETY: `windowed::run_window` hands the backend to `ck_window::run`, which holds the window (and so
        // its `CAMetalLayer`) until the presenter's end is proven (its teardown destroys the window only after
        // a `Proven` end, and never otherwise), which is the contract of `MetalBackend::with_resources`; the
        // backend is used as nothing but that run's presenter.
        let metal = unsafe {
            MetalBackend::with_resources(
                Arc::new(move || notifier.wake()),
                service,
                env.wake_relay.clone(),
                context,
                limits,
            )
        };
        let presenter = Renderer::new(
            Reporting {
                inner: metal,
                pending: false,
                notes: Rc::clone(&env.notes),
                metrics: env.metrics.clone(),
                reported: false,
                resources: service_for_metrics,
            },
            Arc::clone(&env.bridge),
        );
        windowed::run_window(
            &env,
            CocoaBackend::new,
            presenter,
            &RESOURCES,
            windowed::DRAIN,
        )
    })
}

/// The `--metal-smoke` check: one tiny GPU computation, verified.
pub(crate) fn smoke() -> Result<(), String> {
    let mut device = ck_gpu_metal::Device::new().map_err(|e| e.to_string())?;
    let values = device.probe().map_err(|e| e.to_string())?;
    for (index, actual) in values.iter().enumerate() {
        let expected = u32::try_from(index).map_err(|e| e.to_string())? * 3 + 7;
        if *actual != expected {
            return Err(format!(
                "Metal result {index}: expected {expected}, got {actual}"
            ));
        }
    }
    println!("Metal smoke passed: {} GPU results verified", values.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, PoisonError};

    use ck_runtime::RunEnd;
    use ck_window::Notifier;

    use crate::mode::{Backend, Mode};
    use crate::windowed::{Plan, Setup};
    use crate::{Shared, config, native, supervise_with_owner_factory, testing, vm};

    /// The run through the real composition with the native run closure, built with `factory` for the device context. The
    /// script is the UI VM's; `ui_log` holds what it left (a file per callback that ran).
    fn run_with_context(
        factory: impl FnOnce() -> Result<Arc<ck_gpu_metal::MetalContext>, String> + 'static,
    ) -> (crate::Status, Option<String>, testing::Dir) {
        let dir = testing::Dir::new();
        let touched = |name: &str| {
            format!(
                "io.open([==[{}]==], 'w'):close()",
                dir.0.join(name).display()
            )
        };
        let ui = dir.0.join("ui.lua");
        std::fs::write(
            &ui,
            format!(
                "window.open{{title='t', width=100, height=80, clear={{0, 0, 0}}}}\n\
                 window.on_opened(function() {} end)\n\
                 function startup() {} end\n\
                 function shutdown() {} end\n",
                touched("opened"),
                touched("startup"),
                touched("shutdown"),
            ),
        )
        .expect("the ui script");
        let config = config::load_text_for_test(&format!(
            "[window]\nbackend = \"native-metal\"\n\n[render]\nassets = {:?}\n\n[lua.ui]\nentry = {ui:?}\n",
            dir.0
        ));
        let ended = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&ended);
        let factory = Mutex::new(Some(factory));
        let status = supervise_with_owner_factory(
            &config,
            Mode::Windowed(Backend::NativeMetal),
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            move |setup: Setup| {
                let factory = factory
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .expect("one run");
                let run = native::run_closure_with(setup.env, factory);
                Plan {
                    extension: setup.extension,
                    run: Some(Box::new(move |context| {
                        let end = run(context);
                        if let RunEnd::Failed(message) = &end {
                            *seen.lock().unwrap_or_else(PoisonError::into_inner) =
                                Some(message.clone());
                        }
                        end
                    })),
                }
            },
            crate::production_headless_plan,
            |wiring, _, _| vm::spawns(&config.vms, &config.whole, Some(wiring)),
        );
        let message = ended.lock().unwrap_or_else(PoisonError::into_inner).clone();
        (status, message, dir)
    }

    #[test]
    fn the_backend_limits_are_the_parsed_resources_and_max_quads_field_for_field() {
        // Non-default values everywhere, so a field wired to another one (or left at its default) shows.
        let config = config::load_text_for_test(
            "[resources]\nmax_prepared_images = 64\nmax_upload_images_per_batch = 8\n\
             max_upload_bytes_per_batch = 4194304\nstaging_bytes = 134217728\ntexture_bytes = 268435456\n\
             max_extra_revisions = 7\nmax_adoptions_per_frame = 5\nbinding_geometry_bytes = 33554432\n",
        );
        let limits = native::backend_limits(&config.resources, 33, 4096);
        assert_eq!(limits.texture.max_prepared_images, 64);
        assert_eq!(limits.texture.max_upload_images_per_batch, 8);
        assert_eq!(limits.texture.max_upload_bytes_per_batch, 4_194_304);
        assert_eq!(limits.texture.staging_bytes, 134_217_728);
        assert_eq!(limits.texture.texture_bytes, 268_435_456);
        assert_eq!(limits.ledger.max_extra_revisions, 7);
        assert_eq!(limits.ledger.max_adoptions_per_frame, 5);
        assert_eq!(limits.binding_geometry_bytes, 33_554_432);
        assert_eq!(limits.max_quads, 33);
        // The bindless table: the setting's slot count, the logical capacity and the extra revisions, field for field.
        let bindless = limits.bindless;
        assert_eq!(bindless.texture_slots, 4096);
        assert_eq!(bindless.max_textures, config.resources.service.max_textures);
        assert_eq!(bindless.max_extra_revisions, 7);
    }

    #[test]
    fn a_missing_metal_device_fails_the_run_before_any_window_and_the_ui_still_runs_its_shutdown() {
        // The failure is a presenter start failure's text and no window is opened: the script's `on_opened` (the `opened`
        // file) never runs, while start-up and the UI's `shutdown()` do. Reached only on a Mac without a Metal device, so
        // it is tested through the context factory.
        let (status, message, dir) = run_with_context(|| Err("no Metal device".to_owned()));
        assert_eq!(status.code, 1, "{message:?}");
        assert_eq!(
            message.as_deref(),
            Some("cannot start the presenter: no Metal device"),
            "the failure is a presenter start failure's text"
        );
        assert!(dir.0.join("startup").exists(), "start-up ran");
        assert!(
            !dir.0.join("opened").exists(),
            "no window was opened, so `Opened` was never posted"
        );
        assert!(
            dir.0.join("shutdown").exists(),
            "the UI's shutdown() ran in the normal teardown"
        );
    }
}
