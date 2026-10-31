//! The scene a `MetalBackend::with_resources` backend draws (milestone 5, issue 5b-2): a published frame and the metrics it
//! was made with, as the Metal presenter's [`MetalSceneSource`].
//!
//! Publishing is O(1): the sink only wraps the snapshot's `Arc<SceneFrame>` and its [`Metrics`]. Placement in pixels happens
//! in [`place`](MetalSceneSource::place), which the presenter calls **on its render thread at frame build**, so nothing in
//! proportion to the scene runs under the bridge's lock.

use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_gpu_metal::{DestinationRect, InstanceRole, MetalInstance, MetalSceneSource};
use ck_resources::TextureKey;

use crate::{Metrics, SceneFrame};

/// An immutable scene frame with the metrics of the snapshot that carried it.
///
/// **The size the presenter passes to `place` is ignored**: CK-110 makes each snapshot carry the metrics it was made with,
/// the legacy quad of the same published frame is placed with those same metrics by the bridge, and so the scene and the
/// legacy quad always agree within one frame. During a resize a snapshot made before the resize is drawn with its own
/// metrics until the resize's own snapshot arrives (the bridge publishes one per metrics change), exactly as the legacy
/// quad is; the presenter's viewport follows the drawable.
#[derive(Debug)]
pub(crate) struct SceneFrameSource {
    frame: Arc<SceneFrame>,
    metrics: Metrics,
    /// Counts the calls to `place` (tests only: placement must never run under the bridge's lock).
    #[cfg(test)]
    placements: Option<Arc<AtomicUsize>>,
}

impl SceneFrameSource {
    pub(crate) fn new(frame: Arc<SceneFrame>, metrics: Metrics) -> Self {
        Self {
            frame,
            metrics,
            #[cfg(test)]
            placements: None,
        }
    }

    /// The same, counting its `place` calls in `counter` (tests only).
    #[cfg(test)]
    pub(crate) fn counting(
        frame: Arc<SceneFrame>,
        metrics: Metrics,
        counter: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            frame,
            metrics,
            placements: Some(counter),
        }
    }
}

impl MetalSceneSource<TextureKey> for SceneFrameSource {
    fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(MetalInstance<TextureKey>)) {
        #[cfg(test)]
        if let Some(counter) = &self.placements {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        // The compatibility quad (the legacy `render.quad`'s) is emitted first, below every scene quad, with its role: the
        // presenter never draws it with the checker (milestone 5, issue 5d-2). A compatibility quad that places to nothing
        // is not emitted (its key still reaches the presenter, as plain data on the frame).
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
            // A quad that places to nothing (clipped away, or without extent in pixels) is not emitted.
            // Placement takes no texture reference (only the key is read), so a frame of N quads costs no atomic traffic on the
            // registry's shared count (CK-148).
            let Some(placed) = quad.placement(&self.metrics) else {
                continue;
            };
            out(MetalInstance {
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
