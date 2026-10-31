//! The render thread's quad steps: reading the slot, retiring and replacing textures, uploading, the device
//! checks, the format notice and the reaping of what the ledger has proven finished (design §7.2).
//!
//! The render thread owns every texture. A texture is created in a frame that is being drawn (never while frames
//! are stalled), its pixels are copied into a staging buffer and the thread then drops its reference to the pixel
//! source, however the step ends; the copy itself is recorded in the frame's command buffer and counts as done only
//! once that frame is submitted.

use super::{
    CaptureWait, Draw, Fatal, Identity, Owner, Prep, Prepared, QuadParams, QuadSkipped,
    QuadTexture, ReadbackHooks, Replaced, Retirement, Run, Tally, VulkanScene, device_checks,
    failure, release_staging, release_texture, upload_bytes,
};
use crate::capture::FrameCapture;
use crate::draw::QuadPipeline;
use crate::hooks::Event;
use crate::quad::VulkanQuad;
use crate::residency::Admission;
use crate::shared::lock;
use ash::vk;

impl Run<'_> {
    /// Reads the slot (a quad's pixel source is shared by the clone, not copied). `None` without a slot.
    pub(super) fn read_scene(&self) -> Option<VulkanScene> {
        self.quads.as_ref().map(|slot| slot.latest())
    }

    /// Whether quad objects may be destroyed outside the teardown, given the stop state the caller read: only
    /// while the rendering is healthy, so never once a stop was requested, never after a terminal failure (`Failed`
    /// was reported and nothing more is drawn) and never after a device loss. The one decision every opportunistic
    /// disposal makes.
    fn may_dispose(&self, stopping: bool) -> bool {
        !stopping && !self.failed && !self.ledger.is_lost()
    }

    /// [`may_dispose`](Self::may_dispose) with the stop state read now, under its own lock, at the moment of the
    /// decision.
    fn healthy(&self) -> bool {
        self.may_dispose(lock(&self.shared.slot).stop)
    }

    /// A scene without a quad retires the resident texture (and a texture that was never submitted is destroyed at
    /// once), then reaps what the proofs allow.
    pub(super) fn retire_for(&mut self, scene: Option<&VulkanScene>) {
        let Some(scene) = scene else {
            return;
        };
        if scene.quad().is_some() || !self.healthy() {
            return;
        }
        match self.residency.retire_resident() {
            Retirement::DestroyNow => self.destroy_resident(),
            Retirement::Retired => self.retire_objects(),
            Retirement::Nothing | Retirement::Blocked => {}
        }
        self.reap_quad(false);
    }

    /// The resident texture moves to the retired slot.
    fn retire_objects(&mut self) {
        self.gpu.quad.retired = self.gpu.quad.resident.take();
        self.log(Event::QuadTextureRetired);
    }

    /// Destroys the resident texture, which no submitted frame ever used.
    fn destroy_resident(&mut self) {
        if let Some(mut texture) = self.gpu.quad.resident.take() {
            release_texture(
                &self.gpu.device,
                self.gpu.observer.as_deref(),
                self.gpu.quad.pipeline.pool,
                &mut texture,
                false,
            );
        }
    }

    /// Destroys what the ledger's proofs allow and the state permits: a retired texture whose last submit is
    /// proven, and a staging buffer whose upload submit is proven. Decided from the ledger's record of the proof
    /// (a submit number), never from a fence handle. `stopping` is the stop state the caller read.
    pub(super) fn reap_quad(&mut self, stopping: bool) {
        if !self.may_dispose(stopping) {
            return;
        }
        let reapable = self.residency.reapable(self.ledger.proven_through());
        if let Some(owner) = reapable.staging {
            let texture = match owner {
                Owner::Resident => self.gpu.quad.resident.as_mut(),
                Owner::Retired => self.gpu.quad.retired.as_mut(),
            };
            if let Some(texture) = texture {
                release_staging(&self.gpu.device, self.gpu.observer.as_deref(), texture);
            }
            self.residency.staging_destroyed();
        }
        if reapable.retired {
            if let Some(mut texture) = self.gpu.quad.retired.take() {
                release_texture(
                    &self.gpu.device,
                    self.gpu.observer.as_deref(),
                    self.gpu.quad.pipeline.pool,
                    &mut texture,
                    false,
                );
            }
            self.residency.retired_destroyed();
        }
    }

    /// Prepares the quad for a frame that is going to be drawn: the scene's generation, and what to draw.
    pub(super) fn prepare(&mut self, scene: Option<VulkanScene>) -> Result<Prep, Fatal> {
        let Some(scene) = scene else {
            return Ok(Prep::Ready(Prepared::default()));
        };
        let generation = Some(scene.generation());
        let Some(quad) = scene.quad() else {
            return Ok(Prep::Ready(Prepared {
                generation,
                draw: None,
            }));
        };
        if !self.quad_supported {
            self.skip_notice();
            return Ok(Prep::Ready(Prepared {
                generation,
                draw: None,
            }));
        }
        if self.residency.resident_is(quad) {
            let params = QuadParams::of(quad);
            return self.same_image(generation, params, quad);
        }
        self.new_image(scene)
    }

    /// The frame was submitted as `number`: only now is the texture's upload done, and the texture in use by a
    /// submitted frame. The frame's scene generation is what a queued present then reports.
    pub(super) fn quad_submitted(&mut self, number: u64, prepared: Prepared) {
        if let Some(draw) = prepared.draw {
            self.residency.submitted(number, draw.upload);
            if draw.upload
                && let Some(observer) = &self.observer
            {
                observer.quad_upload();
            }
        }
        self.frame_generation = prepared.generation;
        if let Some(chain) = &self.gpu.current
            && self.gpu.captures(chain.extent)
        {
            self.capture_waiting = Some(CaptureWait {
                generation: prepared.generation,
            });
        }
    }

    /// Hands a test the pixels of the frame whose submit was just proven.
    pub(super) fn collect_capture(&mut self) {
        let (Some(CaptureWait { generation }), Some(sink)) =
            (self.capture_waiting.take(), &self.hooks.capture_frames)
        else {
            return;
        };
        if let Some((bytes, size)) = self.gpu.read_capture() {
            lock(sink).push(FrameCapture {
                generation,
                size,
                format: self.gpu.format.format,
                bytes,
            });
        }
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

    /// The device checks for `quad`, before any object is created for it.
    fn check_device(&self, quad: &VulkanQuad) -> Result<(), Fatal> {
        let hooks = ReadbackHooks {
            max_image_dimension: self.hooks.quad_image_limit,
            ..ReadbackHooks::default()
        };
        device_checks(
            &self.gpu.inner.instance,
            self.gpu.selection.physical,
            quad,
            hooks,
        )
        .map_err(|error| Fatal(error.message().to_owned()))
    }

    /// The quad's image is the resident texture's: no upload. A new filter updates the descriptor only while no
    /// pending submitted frame uses it; otherwise the frame waits.
    fn same_image(
        &mut self,
        generation: Option<u64>,
        params: QuadParams,
        quad: &VulkanQuad,
    ) -> Result<Prep, Fatal> {
        let current = self
            .gpu
            .quad
            .resident
            .as_ref()
            .and_then(QuadTexture::filter);
        if current != Some(params.filter) {
            if !self
                .residency
                .refilter_allowed(self.ledger.proven_through())
            {
                self.log(Event::QuadRefilterWait);
                return Ok(Prep::Wait);
            }
            self.check_device(quad)?;
            let pipeline: &QuadPipeline = &self.gpu.quad.pipeline;
            if let Some(texture) = self.gpu.quad.resident.as_mut() {
                texture.refilter(&self.gpu.device, pipeline, params.filter);
            }
        }
        Ok(Prep::Ready(Prepared {
            generation,
            draw: Some(Draw {
                params,
                upload: self.residency.upload_pending(),
            }),
        }))
    }

    /// The quad's image is not the resident texture's: admission, the device checks, the replacement, the new
    /// texture and its staging buffer filled from the pixel source. The scene (and with it this thread's reference
    /// to the pixel source) is dropped as soon as the copy is done, whether it worked or not.
    fn new_image(&mut self, scene: VulkanScene) -> Result<Prep, Fatal> {
        let generation = Some(scene.generation());
        let Some(quad) = scene.quad() else {
            return Ok(Prep::Ready(Prepared {
                generation,
                draw: None,
            }));
        };
        if self.residency.admission() == Admission::Wait {
            if let Some(observer) = &self.observer {
                observer.quad_admission_wait();
            }
            self.log(Event::QuadAdmissionWait);
            return Ok(Prep::Wait);
        }
        if !self.healthy() {
            return Ok(Prep::Wait);
        }
        self.check_device(quad)?;
        let bytes = upload_bytes(quad).map_err(|error| Fatal(error.message().to_owned()))?;
        let params = QuadParams::of(quad);
        match self.residency.replace(Identity::of(quad)) {
            Replaced::Nothing => {}
            Replaced::DestroyOld => self.destroy_resident(),
            Replaced::RetiredOld => self.retire_objects(),
        }
        let mut texture = QuadTexture::with_faults(self.hooks.quad_faults);
        let created = texture.create(
            &self.gpu.device,
            &self.gpu.memory,
            &self.gpu.quad.pipeline,
            quad,
            bytes,
            Tally::default(),
        );
        // The copy into the staging buffer is done: this thread lets go of the pixel source now, however the step
        // ended.
        let size = (params.width, params.height);
        drop(scene);
        self.account_creation(&texture);
        let outcome = created.map_err(|call| self.creation_failure(&texture, call));
        // Kept either way: a partial creation is destroyed with the rest at the proven end.
        self.gpu.quad.resident = Some(texture);
        outcome?;
        self.log(Event::QuadTextureCreated { size });
        Ok(Prep::Ready(Prepared {
            generation,
            draw: Some(Draw {
                params,
                upload: true,
            }),
        }))
    }

    /// The observer's records of a creation (a partial one included).
    fn account_creation(&self, texture: &QuadTexture) {
        let Some(observer) = &self.observer else {
            return;
        };
        if texture.texture_began {
            observer.quad_texture_began();
        }
        if texture.staging_began {
            observer.quad_staging_began();
        }
        if let Some(size) = texture.texture_bytes {
            observer.quad_allocation("texture", size);
        }
        if let Some(size) = texture.staging_bytes {
            observer.quad_allocation("staging", size);
        }
    }

    /// A failed creation as a `Failed`: an allocation names its size.
    fn creation_failure(
        &mut self,
        texture: &QuadTexture,
        (call, code): (&'static str, vk::Result),
    ) -> Fatal {
        if code == vk::Result::ERROR_DEVICE_LOST {
            self.ledger.device_lost();
        }
        if call == "vkAllocateMemory" {
            let (what, size) = if texture.staging_began {
                ("staging buffer", texture.staging_bytes)
            } else {
                ("texture", texture.texture_bytes)
            };
            if let Some(size) = size {
                return Fatal(format!(
                    "allocating {size} bytes for the quad's {what} failed: {code:?}"
                ));
            }
        }
        failure((call, code))
    }
}
