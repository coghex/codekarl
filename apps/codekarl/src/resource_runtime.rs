//! The per-run loader service composed into the runtime after the VMs.

use std::rc::Rc;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ck_resources::{LoaderPool, ResourceService};
use ck_runtime::{Service, ServiceError};

use crate::config::ResourceConfig;
use crate::mode;
use crate::render_api::RenderEnd;
use crate::windowed::RunNotes;

/// Builds the per-mode resource service: a headless run's, or a windowed run's backend service (both backends). The relay
/// already exists before this function is called.
pub(crate) fn compose(
    config: ResourceConfig,
    mode: mode::Mode,
    relay: &ck_render_2d::WakeRelay,
    assets: Option<std::path::PathBuf>,
) -> Result<ResourceService, ck_resources::ServiceError> {
    let service_mode = match mode {
        mode::Mode::Headless => ck_resources::Mode::Headless,
        mode::Mode::Windowed(mode::Backend::NativeMetal | mode::Backend::GlfwVulkan) => {
            ck_resources::Mode::Backend
        }
    };
    let wakes = ck_resources::ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let assets = assets.map(ck_resources::Assets::new);
    ResourceService::with_assets(config.service, service_mode, wakes, assets)
}

/// Owns the loader pool and finishes the resource ledger after every VM owner has stopped.
pub(crate) struct RuntimeService {
    resources: ResourceService,
    pool: Option<LoaderPool>,
    within: Duration,
    notes: Rc<RunNotes>,
    render_end: RenderEnd,
    #[cfg(test)]
    order: Option<Arc<Mutex<Vec<&'static str>>>>,
}

impl RuntimeService {
    pub(crate) fn new(
        resources: ResourceService,
        within: Duration,
        notes: Rc<RunNotes>,
        render_end: RenderEnd,
    ) -> Self {
        Self {
            resources,
            pool: None,
            within,
            notes,
            render_end,
            #[cfg(test)]
            order: None,
        }
    }

    #[cfg(test)]
    fn with_order(mut self, order: Arc<Mutex<Vec<&'static str>>>) -> Self {
        self.order = Some(order);
        self
    }

    #[cfg(test)]
    fn record_order(&self, event: &'static str) {
        if let Some(order) = &self.order {
            order.lock().unwrap().push(event);
        }
    }

    fn finish_without_pool(&self) -> Option<String> {
        self.resources.close();
        let _ = self.resources.finish_teardown();
        let referenced = self.resources.stats().loaders.referenced_at_teardown;
        self.write_final_record(&[]);
        (referenced != 0)
            .then(|| format!("{referenced} resource entries still referenced at finish_teardown"))
    }

    /// The pool could not start: admission closes, the service reconciles, the final record is written and the failure's text
    /// (the spawn error, the loaders left unfinished, the references found) is the service's start failure.
    fn start_failed(&self, error: &ck_resources::SpawnError) -> ServiceError {
        self.resources.close();
        let _ = self.resources.finish_teardown();
        #[cfg(test)]
        self.record_order("finish_teardown");
        let stats = self.resources.stats();
        let unfinished = if error.unfinished.is_empty() {
            String::new()
        } else {
            self.notes.resources_incomplete.set(true);
            format!(
                "; unfinished loader threads: {}",
                error.unfinished.join(", ")
            )
        };
        let referenced = stats.loaders.referenced_at_teardown;
        if referenced != 0 {
            self.notes.resources_incomplete.set(true);
        }
        self.write_final_record(&error.unfinished);
        ServiceError::new(format!(
            "{error}{unfinished}; referenced entries at teardown: {referenced}"
        ))
    }

    /// The run's one final resource record (`debug`, category `resources`): written **after** the reconciliation
    /// (admission closed, the holders released, the pool shut down and `finish_teardown` returned), on every path that
    /// reaches this service's stop and when its start failed. A `Proven` GPU end alone is not that reconciliation; the
    /// referenced count is the one `finish_teardown` left, read from the service. The GPU-side figures were copied to the
    /// run's notes when the presenter ended, before its backend was destroyed.
    fn write_final_record(&self, unfinished: &[String]) {
        #[cfg(test)]
        self.record_order("final_record");
        let stats = self.resources.stats();
        let incomplete = self.notes.resources_incomplete.get();
        let vulkan = *self.notes.vulkan_gpu.borrow();
        let record = if vulkan.is_some() {
            final_record_vulkan(&stats, vulkan.as_ref(), incomplete, unfinished)
        } else {
            let gpu = *self.notes.gpu.borrow();
            final_record(&stats, gpu.as_ref(), incomplete, unfinished)
        };
        crate::lifecycle::emit(ck_log::Level::Debug, "resources", format_args!("{record}"));
    }

    fn finish_after_shutdown(&self, shutdown: &ck_resources::LoaderShutdown) -> Option<String> {
        #[cfg(test)]
        self.record_order("finish_teardown");
        let _ = self.resources.finish_teardown();
        let referenced = self.resources.stats().loaders.referenced_at_teardown;
        let incomplete = !shutdown.unfinished.is_empty() || referenced != 0;
        if incomplete {
            self.notes.resources_incomplete.set(true);
        }
        self.write_final_record(&shutdown.unfinished);
        let mut failures = Vec::new();
        if !shutdown.unfinished.is_empty() {
            failures.push(format!(
                "unfinished loader threads: {}",
                shutdown.unfinished.join(", ")
            ));
        }
        if !shutdown.loop_failures.is_empty() {
            failures.push(format!(
                "loader loop failures: {:?}",
                shutdown.loop_failures
            ));
        }
        if referenced != 0 {
            let holders = self.notes.resource_holders.borrow();
            let holder = if holders.is_empty() {
                "unknown holder".to_owned()
            } else {
                holders.join(", ")
            };
            failures.push(format!(
                "{referenced} resource entries still referenced at finish_teardown by {holder}"
            ));
        }
        (!failures.is_empty()).then(|| failures.join("; "))
    }
}

/// The text of the final resource record: textures created by the upload store (GPU runs; 0 without one), what the registry
/// still holds after the reconciliation (live and retiring entries, and the entries `finish_teardown` found referenced), the
/// CPU pixel budget's peak and the refused admissions of the three budgets, the presenter's measured GPU bytes and the most
/// checker instances one committed frame drew. `incomplete` says the run's own resource note was set (loaders or references
/// outlived the stop deadline), which is the only case where a non-zero referenced count is expected.
pub(crate) fn final_record(
    stats: &ck_resources::ServiceStats,
    gpu: Option<&crate::windowed::GpuFigures>,
    incomplete: bool,
    unfinished: &[String],
) -> String {
    let gpu_text = gpu.map_or_else(
        || "no presenter".to_owned(),
        |gpu| {
            format!(
                "textures created {}, frames {}, bindless_slots={}, upload texture peak {} B, upload staging peak {} B, checker texture {} B, checker staging {} B, instance buffer {} B, checker instances (most in one frame) {}",
                gpu.textures_created,
                gpu.frames,
                gpu.bindless_slots,
                gpu.upload_texture_bytes_peak,
                gpu.upload_staging_bytes_peak,
                gpu.checker_texture_bytes,
                gpu.checker_staging_bytes,
                gpu.instance_buffer_bytes,
                gpu.checker_instances_peak
            )
        },
    );
    record_with(stats, &gpu_text, incomplete, unfinished)
}

/// [`final_record`] for a `glfw-vulkan` run: the common part is the same text; the `gpu:` part is the Vulkan presenter's
/// figures and omits the Metal-only ones (bindless slots, instance buffer, checker staging and instances), never as 0.
pub(crate) fn final_record_vulkan(
    stats: &ck_resources::ServiceStats,
    gpu: Option<&crate::windowed::VulkanFigures>,
    incomplete: bool,
    unfinished: &[String],
) -> String {
    let gpu_text = gpu.map_or_else(
        || "no presenter".to_owned(),
        |gpu| {
            format!(
                "textures created {}, frames {}, upload texture peak {} B, upload staging peak {} B, checker texture {} B",
                gpu.textures_created,
                gpu.frames,
                gpu.texture_bytes_peak,
                gpu.staging_bytes_peak,
                gpu.checker_bytes
            )
        },
    );
    record_with(stats, &gpu_text, incomplete, unfinished)
}

/// The record's text around a `gpu:` part.
fn record_with(
    stats: &ck_resources::ServiceStats,
    gpu_text: &str,
    incomplete: bool,
    unfinished: &[String],
) -> String {
    let refused = stats.jobs.refusals + stats.cpu_pixels.refusals + stats.metadata.refusals;
    format!(
        "final resources: {} live, {} retiring, {} referenced at finish_teardown, cpu pixel bytes peak {}, refused admissions {}; gpu: {}; {}",
        stats.registry.live,
        stats.registry.retiring,
        stats.loaders.referenced_at_teardown,
        stats.cpu_pixels.peak,
        refused,
        gpu_text,
        if incomplete { "incomplete" } else { "complete" }
    ) + &if unfinished.is_empty() {
        String::new()
    } else {
        format!("; unfinished loaders: {}", unfinished.join(", "))
    }
}

impl Service for RuntimeService {
    fn name(&self) -> &'static str {
        "resources"
    }

    fn start(&mut self) -> Result<(), ServiceError> {
        match LoaderPool::start(self.resources.clone()) {
            Ok(pool) => {
                self.pool = Some(pool);
                Ok(())
            }
            Err(error) => Err(self.start_failed(&error)),
        }
    }

    fn stop(&mut self) -> Result<(), ServiceError> {
        // Owner stop requests and thread observations have completed before runtime services stop.
        #[cfg(test)]
        self.record_order("resources.stop");
        self.render_end.end();
        let Some(mut pool) = self.pool.take() else {
            return self
                .finish_without_pool()
                .map_or(Ok(()), |message| Err(ServiceError::new(message)));
        };
        #[cfg(test)]
        self.record_order("pool.shutdown.start");
        let shutdown = pool.shutdown(self.within);
        drop(pool);
        #[cfg(test)]
        self.record_order("pool.shutdown.end");
        if let Some(error) = self.finish_after_shutdown(&shutdown) {
            Err(ServiceError::new(error))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::mode::{Backend, Mode};
    use ck_resources::ResourceLimits;
    use ck_runtime::{
        Failure, Outcome, Owner, OwnerError, OwnerSpawn, Progress, Report, RunEnd, Runtime,
        ServiceError,
    };

    fn assert_budgets_zero(service: &ResourceService) {
        let stats = service.stats();
        assert_eq!(stats.jobs.reserved, 0, "job slots");
        assert_eq!(stats.cpu_pixels.reserved, 0, "CPU pixel bytes");
        assert_eq!(stats.metadata.reserved, 0, "metadata bytes");
        assert_eq!(stats.registry.live, 0, "registry slots");
        assert_eq!(
            stats.loaders.referenced_at_teardown, 0,
            "references at teardown"
        );
    }

    fn empty_render_end() -> RenderEnd {
        let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        crate::render_api::extension_with_end(
            None,
            bridge.take_writer().expect("scene writer"),
            None,
            None,
            false,
        )
        .1
    }

    #[test]
    fn composition_uses_the_headless_mode_and_the_backend_mode_for_both_windowed_backends() {
        let relay = ck_render_2d::WakeRelay::default();
        let config = ResourceConfig::default();
        let headless = compose(config, Mode::Headless, &relay, None).expect("headless service");
        assert_eq!(headless.mode(), ck_resources::Mode::Headless);
        assert_eq!(headless.limits(), config.service);
        for backend in [Backend::NativeMetal, Backend::GlfwVulkan] {
            let windowed =
                compose(config, Mode::Windowed(backend), &relay, None).expect("windowed service");
            assert_eq!(windowed.mode(), ck_resources::Mode::Backend, "{backend:?}");
            assert_eq!(windowed.limits(), config.service, "{backend:?}");
        }
    }

    #[test]
    fn native_composition_routes_upload_and_retirement_wakes_through_the_relay() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("checker.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write checker asset");
        let relay = ck_render_2d::WakeRelay::default();
        let service = compose(
            ResourceConfig::default(),
            Mode::Windowed(Backend::NativeMetal),
            &relay,
            Some(assets.0.clone()),
        )
        .expect("native service");
        let key = service.load("checker.png").expect("admit checker");
        let job = service.take_job().expect("checker load job");
        service.complete(job.run());
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&wakes);
        relay.connect(Arc::new(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        // A backend-mode completion notifies the upload wake. The relay keeps that wake sticky until
        // the presenter exists, then delivers it exactly once.
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
        let retained = service.retain(key).expect("retain checker for retirement");
        service.release(key).expect("release checker owner");
        drop(retained);
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the configured retirement wake also reaches the relay"
        );
        service.close();
        let _ = service.finish_teardown();
    }

    #[test]
    fn headless_composition_reads_from_the_configured_assets_root() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("loaded.png"),
            crate::testing::png(1, 1, &[5, 6, 7, 255]),
        )
        .expect("write configured asset");
        let service = compose(
            ResourceConfig::default(),
            Mode::Headless,
            &ck_render_2d::WakeRelay::default(),
            Some(assets.0.clone()),
        )
        .expect("compose headless resources");
        let key = service.load("loaded.png").expect("admit configured asset");
        let job = service.take_job().expect("take asset load");
        service.complete(job.run());
        assert_eq!(
            service.status(key).expect("loaded resource").state,
            ck_resources::StatusState::CpuReady
        );
        service.close();
        let _ = service.finish_teardown();
    }

    #[test]
    fn a_loader_result_returning_after_close_is_discarded() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("late.png"),
            crate::testing::png(1, 1, &[5, 6, 7, 255]),
        )
        .expect("write configured asset");
        let service = compose(
            ResourceConfig::default(),
            Mode::Headless,
            &ck_render_2d::WakeRelay::default(),
            Some(assets.0.clone()),
        )
        .expect("compose headless resources");
        let key = service.load("late.png").expect("admit asset");
        let job = service.take_job().expect("take loader job");
        service.close();
        service.complete(job.run());
        assert_eq!(service.stats().jobs.reserved, 0);
        assert_eq!(service.check(key), Err(ck_resources::HandleError::Closed));
        let mut uploads = Vec::new();
        service.take_uploads(&mut uploads, usize::MAX);
        assert!(uploads.is_empty(), "a late result must not become visible");
        let _ = service.finish_teardown();
    }

    struct ShutdownLoad {
        service: ResourceService,
        seen: Arc<Mutex<bool>>,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Owner for ShutdownLoad {
        fn name(&self) -> &'static str {
            "ui"
        }
        fn wait_ready(&mut self, _: Duration) -> Result<(), OwnerError> {
            Ok(())
        }
        fn poll_ready(&mut self, _: Duration) -> Result<Progress, OwnerError> {
            Ok(Progress::Done)
        }
        fn request_start(&mut self) -> Result<(), OwnerError> {
            Ok(())
        }
        fn await_started(&mut self, _: Duration) -> Result<(), OwnerError> {
            Ok(())
        }
        fn poll_started(&mut self, _: Duration) -> Result<Progress, OwnerError> {
            Ok(Progress::Done)
        }
        fn request_stop(&mut self) -> Result<(), OwnerError> {
            self.order.lock().unwrap().push("ui.shutdown");
            *self.seen.lock().unwrap() = match self.service.load("late.png") {
                Err(ck_resources::AdmitError::Ended) => true,
                Ok(key) => {
                    self.service
                        .release(key)
                        .expect("release negative-control load");
                    false
                }
                Err(error) => panic!("unexpected admission result: {error}"),
            };
            Ok(())
        }
        fn await_stopped(&mut self, _: Duration) -> Result<(), OwnerError> {
            Ok(())
        }
        fn cancel(&mut self) {}
        fn wait_finished(&mut self, _: Duration) -> bool {
            true
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the four teardown orders and their final record share one composition"
    )]
    fn run_closes_admission_before_vm_shutdown_in_headless_and_native_fake_modes() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("scene.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write scene texture");
        for mode in [ck_resources::Mode::Headless, ck_resources::Mode::Backend] {
            for with_close in [true, false] {
                let service = ResourceService::with_assets(
                    ResourceLimits::DEFAULT,
                    mode,
                    ck_resources::ServiceWakes::default(),
                    Some(ck_resources::Assets::new(&assets.0)),
                )
                .expect("resource service");
                let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
                let key = service.load("scene.png").expect("admit scene texture");
                let job = service.take_job().expect("take scene texture job");
                service.complete(job.run());
                let mut writer = bridge.take_writer().expect("scene writer");
                writer
                    .create_quad(
                        service.retain(key).expect("retain scene texture"),
                        0.0,
                        0.0,
                        1.0,
                        1.0,
                    )
                    .expect("add scene texture");
                service.release(key).expect("release public texture owner");
                assert!(writer.publish(), "publish scene texture");
                let (_, render_end) =
                    crate::render_api::extension_with_end(None, writer, None, None, false);
                let notes = Rc::new(RunNotes::default());
                let observed = Arc::new(Mutex::new(false));
                let order = Arc::new(Mutex::new(Vec::new()));
                let owner_service = service.clone();
                let owner_observed = Arc::clone(&observed);
                let owner_order = Arc::clone(&order);
                let runtime_service =
                    RuntimeService::new(service.clone(), Duration::from_secs(1), notes, render_end)
                        .with_order(Arc::clone(&order));
                let runtime = Runtime::new(vec![Box::new(runtime_service)])
                    .with_owners(vec![OwnerSpawn::new("ui", move || {
                        Ok(Box::new(ShutdownLoad {
                            service: owner_service,
                            seen: owner_observed,
                            order: owner_order,
                        }))
                    })])
                    .with_run({
                        let service = service.clone();
                        let bridge = Arc::clone(&bridge);
                        let order = Arc::clone(&order);
                        move |_| {
                            bridge.detach();
                            order.lock().unwrap().push("bridge.detach");
                            if mode == ck_resources::Mode::Backend {
                                service.detach_backend();
                                order.lock().unwrap().push("backend.detach");
                            }
                            if with_close {
                                service.close();
                                order.lock().unwrap().push("service.close");
                            }
                            RunEnd::Finished
                        }
                    });
                let report = runtime.run_report();
                assert!(
                    matches!(report.outcome, Outcome::Clean),
                    "{mode:?}: {:?}; registry {:?}",
                    report.outcome,
                    service.stats().registry
                );
                assert_eq!(
                    *observed.lock().unwrap(),
                    with_close,
                    "the close step refuses shutdown loads; the negative control admits one"
                );
                assert_budgets_zero(&service);
                if with_close {
                    let events = order.lock().unwrap().clone();
                    let position = |event| events.iter().position(|seen| *seen == event).unwrap();
                    assert!(position("bridge.detach") < position("service.close"));
                    assert!(position("service.close") < position("ui.shutdown"));
                    assert!(position("ui.shutdown") < position("resources.stop"));
                    assert!(position("resources.stop") < position("pool.shutdown.start"));
                    assert!(position("pool.shutdown.start") < position("pool.shutdown.end"));
                    assert!(position("pool.shutdown.end") < position("finish_teardown"));
                    assert!(
                        position("finish_teardown") < position("final_record"),
                        "the final record follows the reconciliation: {events:?}"
                    );
                    assert_eq!(
                        events
                            .iter()
                            .filter(|seen| **seen == "final_record")
                            .count(),
                        1,
                        "written once"
                    );
                }
            }
        }
    }

    #[test]
    fn the_final_record_counts_what_is_left_after_the_reconciliation_not_what_the_writer_still_holds()
     {
        // Negative control: the record taken where the run closure returns (before the VMs finished and the holders were
        // released) shows the writer's quad texture still live; the one after the reconciliation shows none.
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("scene.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write scene texture");
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            Some(ck_resources::Assets::new(assets.0.clone())),
        )
        .expect("resource service");
        let decoded = service
            .create_decoded("scene.png")
            .expect("decoded texture");
        let texture = service.retain(decoded.key).expect("scene texture ref");
        service.release(decoded.key).expect("release public owner");
        let early = final_record(&service.stats(), None, false, &[]);
        assert!(
            early.contains("1 live"),
            "the writer still holds it: {early}"
        );
        drop(texture);
        service.close();
        let _ = service.finish_teardown();
        let late = final_record(&service.stats(), None, false, &[]);
        assert!(
            late.contains("0 live,") && late.contains("0 referenced at finish_teardown"),
            "{late}"
        );
        assert!(late.ends_with("complete"), "{late}");
        assert!(
            final_record(&service.stats(), None, true, &[]).ends_with("incomplete"),
            "an incomplete run says so"
        );
        let gpu = crate::windowed::GpuFigures {
            textures_created: 7,
            bindless_slots: 17,
            upload_texture_bytes_peak: 19,
            upload_staging_bytes_peak: 23,
            checker_texture_bytes: 11,
            checker_staging_bytes: 13,
            instance_buffer_bytes: 17,
            checker_instances_peak: 3,
            frames: 5,
        };
        let with_gpu = final_record(&service.stats(), Some(&gpu), false, &[]);
        for needle in [
            "textures created 7",
            "frames 5",
            "bindless_slots=17",
            "upload texture peak 19 B",
            "upload staging peak 23 B",
            "checker texture 11 B",
            "checker staging 13 B",
            "instance buffer 17 B",
            "checker instances (most in one frame) 3",
        ] {
            assert!(with_gpu.contains(needle), "{needle}: {with_gpu}");
        }
    }

    /// The Vulkan presenter's record writes its own figures and omits the Metal-only ones (never as 0); the part before `gpu:` is
    /// byte-identical to the Metal record's for equal service stats. Negative control: the Metal record does carry the omitted
    /// figures, so the omission is the Vulkan record's.
    #[test]
    fn the_vulkan_final_record_writes_its_figures_omits_the_metal_only_ones_and_shares_the_common_part()
     {
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            None,
        )
        .expect("a service");
        service.close();
        let _ = service.finish_teardown();
        let stats = service.stats();
        let vulkan = crate::windowed::VulkanFigures {
            textures_created: 3,
            texture_bytes_peak: 64,
            staging_bytes_peak: 32,
            checker_bytes: 16384,
            frames: 9,
        };
        let metal = crate::windowed::GpuFigures {
            textures_created: 3,
            bindless_slots: 17,
            upload_texture_bytes_peak: 64,
            upload_staging_bytes_peak: 32,
            checker_texture_bytes: 16384,
            checker_staging_bytes: 13,
            instance_buffer_bytes: 17,
            checker_instances_peak: 3,
            frames: 9,
        };
        let on_vulkan = final_record_vulkan(&stats, Some(&vulkan), false, &[]);
        let on_metal = final_record(&stats, Some(&metal), false, &[]);
        let (vulkan_common, vulkan_gpu) = on_vulkan.split_once("; gpu: ").expect("a gpu part");
        let (metal_common, metal_gpu) = on_metal.split_once("; gpu: ").expect("a gpu part");
        assert_eq!(vulkan_common, metal_common, "the common part is identical");
        for needle in [
            "textures created 3",
            "frames 9",
            "upload texture peak 64 B",
            "upload staging peak 32 B",
            "checker texture 16384 B",
        ] {
            assert!(vulkan_gpu.contains(needle), "{needle}: {vulkan_gpu}");
        }
        for omitted in [
            "bindless",
            "checker staging",
            "instance buffer",
            "checker instances",
        ] {
            assert!(
                !vulkan_gpu.contains(omitted),
                "{omitted} is Metal-only: {vulkan_gpu}"
            );
            assert!(
                metal_gpu.contains(omitted),
                "control, {omitted}: {metal_gpu}"
            );
        }
        assert!(vulkan_gpu.ends_with("complete"), "{vulkan_gpu}");
        assert!(
            final_record_vulkan(&stats, None, false, &[]).contains("gpu: no presenter"),
            "a run with no presenter says so"
        );
    }

    #[test]
    fn the_final_record_names_the_references_finish_teardown_found_and_the_unfinished_loaders() {
        // A holder that outlives the reconciliation is reported as referenced (the record never claims zero it did not
        // see), and an unfinished loader is named.
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("scene.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write scene texture");
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            Some(ck_resources::Assets::new(assets.0.clone())),
        )
        .expect("resource service");
        let decoded = service
            .create_decoded("scene.png")
            .expect("decoded texture");
        let held = service.retain(decoded.key).expect("scene texture ref");
        service.release(decoded.key).expect("release public owner");
        service.close();
        let _ = service.finish_teardown();
        let record = final_record(
            &service.stats(),
            None,
            true,
            &["resource loader 1".to_owned()],
        );
        assert!(
            record.contains(" 1 referenced at finish_teardown"),
            "{record}"
        );
        assert!(record.contains("incomplete"), "{record}");
        assert!(
            record.ends_with("; unfinished loaders: resource loader 1"),
            "{record}"
        );
        drop(held);
    }

    #[test]
    fn a_pool_that_cannot_start_reconciles_writes_the_final_record_once_and_names_the_unfinished_loaders()
     {
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            None,
        )
        .expect("resource service");
        let order = Arc::new(Mutex::new(Vec::new()));
        let notes = Rc::new(RunNotes::default());
        let runtime_service = RuntimeService::new(
            service.clone(),
            Duration::from_secs(1),
            Rc::clone(&notes),
            empty_render_end(),
        )
        .with_order(Arc::clone(&order));
        let error = runtime_service.start_failed(&ck_resources::SpawnError {
            index: 1,
            source: std::io::Error::other("no threads"),
            unfinished: vec!["resource loader 0".to_owned()],
        });
        let text = error.to_string();
        assert!(text.contains("cannot start loader thread 1"), "{text}");
        assert!(
            text.contains("unfinished loader threads: resource loader 0"),
            "{text}"
        );
        assert!(service.is_closed(), "admission is closed");
        assert!(notes.resources_incomplete.get());
        assert_eq!(
            *order.lock().unwrap(),
            ["finish_teardown", "final_record"],
            "reconciled, then the one final record"
        );
    }

    #[test]
    fn failed_start_releases_the_parent_scene_holder_before_resource_finish() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("scene.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write scene texture");
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            Some(ck_resources::Assets::new(assets.0.clone())),
        )
        .expect("resource service");
        let decoded = service
            .create_decoded("scene.png")
            .expect("decoded texture");
        let texture = service.retain(decoded.key).expect("scene texture ref");
        service.release(decoded.key).expect("release public owner");
        let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut writer = bridge.take_writer().expect("scene writer");
        writer
            .create_quad(texture, 0.0, 0.0, 1.0, 1.0)
            .expect("editable scene quad");
        assert!(writer.publish(), "publish the scene frame");
        bridge.detach();
        let (_, render_end) =
            crate::render_api::extension_with_end(None, writer, None, None, false);
        let order = Arc::new(Mutex::new(Vec::new()));
        let runtime = Runtime::new(vec![Box::new(
            RuntimeService::new(
                service.clone(),
                Duration::from_secs(1),
                Rc::new(RunNotes::default()),
                render_end,
            )
            .with_order(Arc::clone(&order)),
        )])
        .with_owners(vec![OwnerSpawn::new("ui", || {
            Err("synthetic spawn failure".into())
        })]);
        let report = runtime.run_report();
        assert!(matches!(report.outcome, Outcome::StartFailed { .. }));
        assert_eq!(service.stats().loaders.referenced_at_teardown, 0);
        // An early failure still gets the final record, once, after the reconciliation.
        let events = order.lock().unwrap().clone();
        let position = |event| events.iter().position(|seen| *seen == event);
        assert_eq!(
            events
                .iter()
                .filter(|seen| **seen == "final_record")
                .count(),
            1,
            "{events:?}"
        );
        assert!(
            position("finish_teardown") < position("final_record"),
            "{events:?}"
        );
    }

    #[test]
    fn without_explicit_end_the_parent_scene_holder_survives_resource_finish() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("scene.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write scene texture");
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            Some(ck_resources::Assets::new(assets.0.clone())),
        )
        .expect("resource service");
        let decoded = service
            .create_decoded("scene.png")
            .expect("decoded texture");
        let texture = service.retain(decoded.key).expect("scene texture ref");
        service.release(decoded.key).expect("release public owner");
        let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut writer = bridge.take_writer().expect("scene writer");
        writer
            .create_quad(texture, 0.0, 0.0, 1.0, 1.0)
            .expect("editable scene quad");
        assert!(writer.publish(), "publish the scene frame");
        bridge.detach();
        let (_, render_end) =
            crate::render_api::extension_with_end(None, writer, None, None, false);
        service.close();
        let _ = service.finish_teardown();
        assert_eq!(service.stats().loaders.referenced_at_teardown, 1);
        drop(render_end);
    }

    #[test]
    fn referenced_resource_at_finish_is_named_and_marks_the_run_incomplete() {
        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("held.png"),
            crate::testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write held texture");
        let service = ResourceService::with_assets(
            ResourceLimits::DEFAULT,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            Some(ck_resources::Assets::new(assets.0.clone())),
        )
        .expect("resource service");
        let decoded = service.create_decoded("held.png").expect("decoded texture");
        let held = service.retain(decoded.key).expect("held texture ref");
        service.release(decoded.key).expect("release public owner");

        let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        let (_, render_end) = crate::render_api::extension_with_end(
            None,
            bridge.take_writer().expect("scene writer"),
            None,
            None,
            false,
        );
        let notes = Rc::new(RunNotes::default());
        crate::vm::note_unfinished_holder("ui", false, Some(&notes));
        let runtime_service = RuntimeService::new(
            service.clone(),
            Duration::from_secs(1),
            Rc::clone(&notes),
            render_end,
        );
        service.close();
        let error = runtime_service
            .finish_after_shutdown(&ck_resources::LoaderShutdown {
                finished: vec![],
                unfinished: vec![],
                loop_failures: vec![],
                closed_in: Duration::ZERO,
                waited: Duration::ZERO,
            })
            .expect("held reference is reported");
        assert!(error.contains("1 resource entries"), "{error}");
        assert!(error.contains("UI VM"), "{error}");
        assert_eq!(service.stats().loaders.referenced_at_teardown, 1);
        assert!(notes.resources_incomplete.get());

        let report = Report {
            outcome: Outcome::StopFailed {
                failures: vec![Failure {
                    service: "resources".into(),
                    error: ServiceError::new(error),
                }],
            },
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: false,
        };
        let status = crate::vm::status_of(&report, notes.resources_incomplete.get(), false);
        assert_eq!(status.code, 1, "the stop itself remains a service failure");
        assert!(status.owners_incomplete);
        assert_eq!(
            crate::lifecycle::decide(&ck_log::FlushOutcome::Complete, status),
            crate::lifecycle::Exit::Immediately,
            "the executable maps the incomplete owner/resource state to exit 3"
        );
        drop(held);
    }

    #[test]
    fn loader_deadline_is_incomplete_but_a_finished_loop_failure_is_ordinary() {
        let mk_service = || {
            ResourceService::with_assets(
                ResourceLimits::DEFAULT,
                ck_resources::Mode::Backend,
                ck_resources::ServiceWakes::default(),
                None,
            )
            .expect("resource service")
        };
        let report = |error: String| Report {
            outcome: Outcome::StopFailed {
                failures: vec![Failure {
                    service: "resources".into(),
                    error: ServiceError::new(error),
                }],
            },
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: false,
        };

        let notes = Rc::new(RunNotes::default());
        let stalled = RuntimeService::new(
            mk_service(),
            Duration::ZERO,
            Rc::clone(&notes),
            empty_render_end(),
        );
        let error = stalled
            .finish_after_shutdown(&ck_resources::LoaderShutdown {
                finished: vec![],
                unfinished: vec!["ck-loader-0".into()],
                loop_failures: vec![],
                closed_in: Duration::ZERO,
                waited: Duration::ZERO,
            })
            .expect("deadline failure");
        let incomplete =
            crate::vm::status_of(&report(error), notes.resources_incomplete.get(), false);
        assert_eq!(
            crate::lifecycle::decide(&ck_log::FlushOutcome::Complete, incomplete),
            crate::lifecycle::Exit::Immediately,
            "a loader past the deadline selects exit 3"
        );

        let notes = Rc::new(RunNotes::default());
        let failed_loop = RuntimeService::new(
            mk_service(),
            Duration::ZERO,
            Rc::clone(&notes),
            empty_render_end(),
        );
        let error = failed_loop
            .finish_after_shutdown(&ck_resources::LoaderShutdown {
                finished: vec!["ck-loader-0".into()],
                unfinished: vec![],
                loop_failures: vec![ck_resources::LoopFailure {
                    thread: "ck-loader-0".into(),
                    message: "injected loop failure".into(),
                }],
                closed_in: Duration::ZERO,
                waited: Duration::ZERO,
            })
            .expect("loop failure");
        let ordinary = crate::vm::status_of(&report(error), false, false);
        assert_eq!(
            crate::lifecycle::decide(&ck_log::FlushOutcome::Complete, ordinary),
            crate::lifecycle::Exit::Return(1),
            "a loader loop failure after the thread finished selects exit 1"
        );
    }
}
