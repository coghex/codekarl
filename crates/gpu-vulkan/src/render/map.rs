//! The map steps both render threads share (milestone 6, slice 1): preparing a frame's map pass, making the input and the target
//! resident, retiring what they replace, and reaping what the ledger's proofs allow. The data is [`map`](crate::map)'s, the
//! bookkeeping [`map_residency`](crate::map_residency)'s and the objects and the recording [`gpu_map`](crate::gpu_map)'s. The scene
//! presenter and the quad-slot presenter lend this driver their device state for each call and keep only the format notice and the
//! stop rules for themselves.
//!
//! The render thread owns every map object. They are created in a frame that is being drawn (never while frames are stalled), the
//! input's pixels are copied into a staging buffer and the thread then drops its reference to them however the step ends, and the
//! upload is recorded in the frame's command buffer and counts as done only once that frame is submitted. A replaced input or
//! target is retired and destroyed once the ledger has the proof of the last submit that used it; a replacement that would retire
//! a holder while a retired one of its kind still waits makes the frame wait, and nothing is made.
//!
//! **A refused allocation is not a failure of the run.** The new objects are made first, into temporaries; if the device cannot give
//! the memory, what was made is destroyed (no submit ever used it), the books stay as they were, the frame is drawn without the map
//! (never with a stale target), the publisher is told once per episode ([`MapRefusalSink`](crate::MapRefusalSink)) and the next
//! frame asks again, one attempt per frame. The first attempt that succeeds ends the episode.

use ash::vk;

use crate::gpu::Gpu;
use crate::gpu_map::{MapInputObjects, MapRecord, MapTargetObjects, device_checks};
use crate::hooks::{Observer, TestHooks};
use crate::ledger::Ledger;
use crate::map::VulkanMap;
use crate::map_residency::{Admission, Displaced, MapResidency, Need};
use crate::render::{Fatal, Prepared, failure};

/// What preparing a frame's map decided.
pub(in crate::render) enum MapPrep {
    /// The frame has no map pass (no map, an unsupported surface format, or a refused allocation).
    Nothing,
    /// The frame waits, its redraw kept.
    Wait,
    /// The frame records this map pass.
    Ready(MapRecord),
}

/// What a presenter thread lends the driver for one call.
pub(in crate::render) struct MapContext<'a> {
    pub(in crate::render) gpu: &'a mut Gpu,
    pub(in crate::render) ledger: &'a mut Ledger,
    pub(in crate::render) hooks: &'a mut TestHooks,
    pub(in crate::render) observer: Option<&'a Observer>,
    /// Objects may be destroyed now (the rendering is healthy), the stop state not yet read.
    pub(in crate::render) may_dispose: bool,
    /// Objects may be made and destroyed now: the rendering is healthy and no stop was requested (read at the decision).
    pub(in crate::render) healthy: bool,
}

/// The map's bookkeeping and the state of its refusal episode.
#[derive(Default)]
pub(in crate::render) struct MapDriver {
    residency: MapResidency,
    /// The last attempt to make the map's objects was refused (and the publisher told): the first success is told too.
    refusing: bool,
    /// The last attempt to prepare the map was refused: the render thread keeps a redraw wanted. Unlike `refusing` (the episode the
    /// publisher is told about), it ends with any step that does not itself refuse, a frame without a map included.
    retry: bool,
}

impl MapDriver {
    /// Whether the last attempt to make the map's objects was refused: the render thread then keeps a redraw wanted, so the next
    /// normally paced frame (the acquire and the present pace it: one attempt per frame) asks again with nothing else changing. The
    /// first success ends it, and the demand-driven idle resumes.
    pub(in crate::render) fn retry_wanted(&self) -> bool {
        self.retry
    }

    /// A frame that is going to be drawn does not attempt the map (it has none, or its surface cannot draw it): nothing is refused,
    /// so no retry is wanted. The episode stays open for the publisher until an attempt succeeds.
    pub(in crate::render) fn not_attempted(&mut self) {
        self.retry = false;
    }

    /// Prepares the map of a frame that is going to be drawn (the surface format was checked by the caller).
    pub(in crate::render) fn prepare(
        &mut self,
        mut cx: MapContext<'_>,
        map: &VulkanMap,
    ) -> Result<MapPrep, Fatal> {
        self.retry = false;
        device_checks(
            &cx.gpu.inner.instance,
            cx.gpu.selection.physical,
            map,
            cx.hooks.quad_image_limit,
        )
        .map_err(|error| Fatal(error.message().to_owned()))?;
        if !cx.gpu.map.pipeline.exists() {
            cx.gpu.create_map_pipeline().map_err(|call| {
                if call.1 == vk::Result::ERROR_DEVICE_LOST {
                    cx.ledger.device_lost();
                }
                failure(call)
            })?;
        }
        // What the proofs allow goes first, so a retired holder whose submit is proven does not make this frame wait.
        self.reap(cx.gpu, cx.ledger, cx.observer, cx.may_dispose);
        let input = map.input();
        let need = self.residency.need(input.generation(), map.view());
        if need.input || need.target {
            if self.residency.admission(need) == Admission::Wait {
                if let Some(observer) = cx.observer {
                    observer.map_admission_wait();
                }
                return Ok(MapPrep::Wait);
            }
            if !cx.healthy {
                return Ok(MapPrep::Wait);
            }
            if let Some(reason) = self.replace(&mut cx, map, need)? {
                // Refused, not failed: this frame is drawn without the map (never with a stale target), the publisher is told
                // once per episode, and the next frame asks again.
                if let Some(refusals) = map.refusals() {
                    refusals.refused(&reason);
                }
                self.refusing = true;
                self.retry = true;
                return Ok(MapPrep::Nothing);
            }
        }
        if self.refusing {
            if let Some(refusals) = map.refusals() {
                refusals.cleared();
            }
            self.refusing = false;
        }
        Ok(MapPrep::Ready(MapRecord::of(
            map,
            self.residency.upload_pending(),
        )))
    }

    /// The new input and/or target replace the resident ones. The new objects are made first, into temporaries: a refusal (the
    /// device is out of memory) destroys what was made and changes nothing else, so the resident objects and the books stay as they
    /// were. Only when both exist do the bookkeeping, the retirement of the old objects and the install follow. `Ok(Some(reason))`
    /// is a refusal; an error that is not about memory is a `Failed`.
    fn replace(
        &mut self,
        cx: &mut MapContext<'_>,
        map: &VulkanMap,
        need: Need,
    ) -> Result<Option<String>, Fatal> {
        let mut faults = cx.hooks.quad_faults;
        if need.input || need.target {
            if cx.hooks.map_allocation_skip > 0 {
                // A test lets this many creations through before its refusals begin.
                cx.hooks.map_allocation_skip -= 1;
            } else if cx.hooks.map_allocation_failures > 0 {
                cx.hooks.map_allocation_failures -= 1;
                faults.texture_allocation = true;
            }
        }
        let pool = cx.gpu.map.pipeline.pool;
        let mut made_input = None;
        if need.input {
            let mut objects = MapInputObjects::default();
            let created = objects.create(
                &cx.gpu.device,
                &cx.gpu.memory,
                &cx.gpu.map.pipeline,
                map.input(),
                faults,
            );
            if let Err(call) = created {
                objects.destroy(&cx.gpu.device, pool);
                return Self::creation_failure(cx.ledger, "input", call);
            }
            made_input = Some(objects);
        }
        let mut made_target = None;
        if need.target {
            let mut objects = MapTargetObjects::default();
            let created = objects.create(
                &cx.gpu.device,
                &cx.gpu.memory,
                &cx.gpu.map.pipeline,
                &cx.gpu.quad.pipeline,
                map.view(),
                (faults, vk::ImageUsageFlags::empty()),
            );
            if let Err(call) = created {
                objects.destroy(&cx.gpu.device, pool);
                if let Some(mut input) = made_input {
                    input.destroy(&cx.gpu.device, pool);
                }
                return Self::creation_failure(cx.ledger, "target", call);
            }
            made_target = Some(objects);
        }
        let replaced = self
            .residency
            .replace(need, map.input().generation(), map.view());
        let device = &cx.gpu.device;
        if replaced.input == Displaced::DestroyNow
            && let Some(mut old) = cx.gpu.map.input.take()
        {
            old.destroy(device, pool);
            if let Some(observer) = cx.observer {
                observer.map_input_released(false);
            }
        }
        if replaced.input == Displaced::Retired {
            cx.gpu.map.retired_input = cx.gpu.map.input.take();
            if let Some(observer) = cx.observer {
                observer.map_retired();
            }
        }
        if replaced.target == Displaced::DestroyNow
            && let Some(mut old) = cx.gpu.map.target.take()
        {
            old.destroy(device, pool);
            if let Some(observer) = cx.observer {
                observer.map_target_released(false);
            }
        }
        if replaced.target == Displaced::Retired {
            cx.gpu.map.retired_target = cx.gpu.map.target.take();
            if let Some(observer) = cx.observer {
                observer.map_retired();
            }
        }
        if let Some(input) = made_input {
            cx.gpu.map.input = Some(input);
            if let Some(observer) = cx.observer {
                observer.map_input_began();
            }
        }
        if let Some(target) = made_target {
            cx.gpu.map.target = Some(target);
            if let Some(observer) = cx.observer {
                observer.map_target_began(map.view());
            }
        }
        Ok(None)
    }

    /// A failed creation: a refusal when the device could not give the memory (the frame goes on without the map and the next one
    /// asks again), a `Failed` for anything else.
    fn creation_failure(
        ledger: &mut Ledger,
        what: &str,
        (call, code): (&'static str, vk::Result),
    ) -> Result<Option<String>, Fatal> {
        if code == vk::Result::ERROR_DEVICE_LOST {
            ledger.device_lost();
        }
        if matches!(
            code,
            vk::Result::ERROR_OUT_OF_HOST_MEMORY
                | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY
                | vk::Result::ERROR_TOO_MANY_OBJECTS
                | vk::Result::ERROR_OUT_OF_POOL_MEMORY
                | vk::Result::ERROR_FRAGMENTED_POOL
                | vk::Result::ERROR_FEATURE_NOT_PRESENT
        ) {
            return Ok(Some(format!(
                "the map's {what} could not be allocated ({call} failed: {code:?}); the frame is drawn without the map and the next frame tries again"
            )));
        }
        Err(failure((call, code)))
    }

    /// The frame was submitted as `number`: the map's input and target are in use by it, and an input whose upload it carried is
    /// uploaded.
    pub(in crate::render) fn submitted(
        &mut self,
        number: u64,
        prepared: &Prepared,
        observer: Option<&Observer>,
    ) {
        if let Some(record) = prepared.map {
            self.residency.submitted(number, record.upload);
            if let Some(observer) = observer {
                observer.map_submitted(record.upload);
            }
        }
    }

    /// Destroys what the ledger's proofs allow and the state permits: a retired input or target whose last submit is proven, and the
    /// staging buffers of an upload whose submit is proven. Decided from the ledger's record of the proof (a submit number), never
    /// from a fence handle. `may_dispose` is the caller's rule for opportunistic disposal (never once a stop was requested).
    pub(in crate::render) fn reap(
        &mut self,
        gpu: &mut Gpu,
        ledger: &Ledger,
        observer: Option<&Observer>,
        may_dispose: bool,
    ) {
        if !may_dispose {
            return;
        }
        let reapable = self.residency.reapable(ledger.proven_through());
        let pool = gpu.map.pipeline.pool;
        if reapable.staging {
            if let Some(input) = gpu.map.input.as_mut() {
                input.destroy_staging(&gpu.device);
            }
            self.residency.staging_destroyed();
        }
        if reapable.input {
            if let Some(mut input) = gpu.map.retired_input.take() {
                input.destroy(&gpu.device, pool);
                if let Some(observer) = observer {
                    observer.map_input_released(true);
                }
            }
            self.residency.retired_input_destroyed();
        }
        if reapable.target {
            if let Some(mut target) = gpu.map.retired_target.take() {
                target.destroy(&gpu.device, pool);
                if let Some(observer) = observer {
                    observer.map_target_released(true);
                }
            }
            self.residency.retired_target_destroyed();
        }
    }
}
