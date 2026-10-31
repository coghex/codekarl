//! The render thread's quad objects: the pipeline and the (at most two) textures, with the observer's records of
//! their creation and destruction. Like the rest of [`Gpu`](crate::gpu::Gpu) there is no `Drop`: only an explicit
//! destroy, and only once the ledger has the proof, releases anything.

use ash::vk;

use crate::draw::{QuadParams, QuadPipeline, QuadTexture, Tally, record_draw};
use crate::gpu::{Failure, Gpu};
use crate::hooks::{Event, Observer, PipelineFault};

/// What the render thread created for the quad. Textures are bookkept by [`Residency`](crate::residency::Residency);
/// these are the objects behind that bookkeeping.
#[derive(Default)]
pub(crate) struct QuadObjects {
    pub(crate) pipeline: QuadPipeline,
    pub(crate) resident: Option<QuadTexture>,
    pub(crate) retired: Option<QuadTexture>,
}

impl QuadObjects {
    /// Whether any quad object exists.
    pub(crate) fn any(&self) -> bool {
        self.pipeline.exists() || self.resident.is_some() || self.retired.is_some()
    }
}

/// The surface formats textured drawing is exact on (design §7.3).
pub(crate) fn format_supported(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::B8G8R8A8_UNORM | vk::Format::R8G8B8A8_UNORM
    )
}

/// Destroys a texture (its staging buffer included) and records it. Only when nothing submitted uses it.
pub(crate) fn release_texture(
    device: &ash::Device,
    observer: Option<&Observer>,
    pool: Option<vk::DescriptorPool>,
    texture: &mut QuadTexture,
    teardown: bool,
) {
    let had_staging = texture.has_staging();
    let had_texture = texture.texture_began;
    texture.destroy(device, pool);
    if let Some(observer) = observer {
        if had_staging {
            observer.quad_staging_released();
            observer.push(Event::QuadStagingDestroyed { teardown });
        }
        if had_texture {
            observer.quad_texture_released();
            observer.push(Event::QuadTextureDestroyed { teardown });
        }
    }
}

/// Destroys only a texture's staging buffer (its upload is proven finished) and records it.
pub(crate) fn release_staging(
    device: &ash::Device,
    observer: Option<&Observer>,
    texture: &mut QuadTexture,
) {
    let had_staging = texture.has_staging();
    texture.destroy_staging(device);
    if had_staging && let Some(observer) = observer {
        observer.quad_staging_released();
        observer.push(Event::QuadStagingDestroyed { teardown: false });
    }
}

impl Gpu {
    /// Creates the quad pipeline, its layouts, pool and samplers against the render pass; `fault` makes the
    /// pipeline's own creation fail (a test). Whatever exists when it fails is destroyed by [`destroy_all`](Self::destroy_all).
    pub(crate) fn create_quad_pipeline(
        &mut self,
        fault: Option<PipelineFault>,
    ) -> Result<(), Failure> {
        let Some(render_pass) = self.render_pass else {
            return Err((
                "vkCreateGraphicsPipelines",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        self.quad
            .pipeline
            .create_with_fault(&self.device, render_pass, Tally::default(), fault)?;
        if let Some(observer) = &self.observer {
            observer.push(Event::QuadPipelineCreated);
        }
        Ok(())
    }

    /// Destroys every quad object, textures first and the pipeline after: the teardown's, once the end is proven.
    pub(crate) fn destroy_quad(&mut self) {
        let mut objects = std::mem::take(&mut self.quad);
        for mut texture in [objects.retired.take(), objects.resident.take()]
            .into_iter()
            .flatten()
        {
            release_texture(
                &self.device,
                self.observer.as_deref(),
                objects.pipeline.pool,
                &mut texture,
                true,
            );
        }
        objects.pipeline.destroy(&self.device);
    }

    /// Records the quad's draw into the frame's render pass.
    pub(crate) fn record_quad_draw(
        &self,
        cmd: vk::CommandBuffer,
        params: &QuadParams,
        target: (u32, u32),
    ) {
        if let Some(texture) = &self.quad.resident {
            record_draw(
                &self.device,
                cmd,
                &self.quad.pipeline,
                texture,
                params,
                target,
            );
        }
    }

    /// Records the resident texture's upload, before the render pass.
    pub(crate) fn record_quad_upload(&self, cmd: vk::CommandBuffer) {
        if let Some(texture) = &self.quad.resident {
            texture.record_upload(&self.device, cmd);
            if let Some(observer) = &self.observer {
                observer.push(Event::QuadUploadRecorded);
            }
        }
    }
}
