//! A Vulkan frame's scene source: shared scene data and coherent placement metrics.

use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_gpu_vulkan::{DestinationRect, InstanceRole, VulkanInstance, VulkanSceneSource};
use ck_resources::TextureKey;

use crate::{Metrics, SceneFrame};

/// An immutable scene frame and the metrics of the snapshot that carried it.
#[derive(Debug)]
pub(crate) struct VulkanSceneFrame {
    frame: Arc<SceneFrame>,
    metrics: Metrics,
    #[cfg(test)]
    placements: Option<Arc<AtomicUsize>>,
}

impl VulkanSceneFrame {
    pub(crate) fn new(frame: Arc<SceneFrame>, metrics: Metrics) -> Self {
        Self {
            frame,
            metrics,
            #[cfg(test)]
            placements: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn counting(
        frame: Arc<SceneFrame>,
        metrics: Metrics,
        placements: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            frame,
            metrics,
            placements: Some(placements),
        }
    }
}

impl VulkanSceneSource<TextureKey> for VulkanSceneFrame {
    fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(VulkanInstance<TextureKey>)) {
        #[cfg(test)]
        if let Some(placements) = &self.placements {
            placements.fetch_add(1, Ordering::SeqCst);
        }

        let compat = self
            .frame
            .compat()
            .into_iter()
            .map(|quad| (quad, InstanceRole::Compat));
        let scene = self
            .frame
            .quads()
            .iter()
            .map(|quad| (quad, InstanceRole::Scene));
        for (quad, role) in compat.chain(scene) {
            let Some(placed) = quad.placement(&self.metrics) else {
                continue;
            };
            out(VulkanInstance {
                key: quad.texture.key(),
                destination: DestinationRect {
                    left: placed.destination.left,
                    top: placed.destination.top,
                    right: placed.destination.right,
                    bottom: placed.destination.bottom,
                },
                source: [
                    placed.source.left,
                    placed.source.top,
                    placed.source.right,
                    placed.source.bottom,
                ],
                role,
            });
        }
    }
}
