//! Thin binding of the backend-neutral resident adapter to CK-156's Vulkan texture ledger.

use std::{collections::VecDeque, sync::Arc};

use ck_gpu_vulkan::{
    Boundary, LedgerEvent, OfferRefusal, PassOutcome, TextureDriver, TextureLedger,
};
use ck_resources::{PixelRevision, ResourceService, TextureKey, UploadRequest};

use crate::resident::{
    LedgerOps, ResidentAdapter, ResidentBoundary, ResidentEvent, ResidentRefusal,
};

struct RevisionPixels(Arc<PixelRevision>);

impl ck_gpu_vulkan::PixelSource for RevisionPixels {
    fn pixels(&self) -> &[u8] {
        self.0.pixels()
    }
}

struct VulkanLedgerOps<'a> {
    ledger: &'a mut TextureLedger<TextureKey>,
    events: &'a mut VecDeque<ResidentEvent>,
}

impl LedgerOps for VulkanLedgerOps<'_> {
    fn room(&self) -> usize {
        self.ledger.room()
    }

    fn extra_room(&self) -> usize {
        self.ledger.extra_room()
    }

    fn offer(&mut self, request: UploadRequest) -> Result<(), ResidentRefusal> {
        let source: Arc<dyn ck_gpu_vulkan::PixelSource> =
            Arc::new(RevisionPixels(Arc::clone(&request.pixels)));
        self.ledger
            .offer(
                request.key,
                request.revision,
                request.pixels.width(),
                request.pixels.height(),
                source,
            )
            .map_err(|refusal| match refusal {
                OfferRefusal::KeyRetiring => ResidentRefusal::KeyRetiring,
                OfferRefusal::Protocol(message) => ResidentRefusal::Protocol(message),
            })
    }

    fn next_event(&mut self) -> Option<ResidentEvent> {
        self.events.pop_front()
    }

    fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    fn commit(&mut self, key: TextureKey, revision: u64) {
        let _ = self.ledger.commit(key, revision);
    }

    fn reject(&mut self, key: TextureKey, revision: u64) {
        let _ = self.ledger.reject(key, revision);
    }

    fn adopt(&mut self, limit: usize, out: &mut Vec<(TextureKey, u64)>) {
        self.ledger.adopt(limit, out);
    }

    fn retire_key(&mut self, key: TextureKey) {
        self.ledger.retire_key(key);
    }
}

/// The Vulkan presenter's `TextureDriver` backed by the shared resident adapter.
pub struct VulkanResidentDriver {
    adapter: ResidentAdapter,
    pending_events: VecDeque<ResidentEvent>,
}

impl std::fmt::Debug for VulkanResidentDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VulkanResidentDriver")
    }
}

impl VulkanResidentDriver {
    /// Creates the driver for a backend-mode resource service.
    #[must_use]
    pub fn new(service: ResourceService, max_adoptions_per_frame: usize) -> Self {
        Self {
            adapter: ResidentAdapter::new(service, max_adoptions_per_frame),
            pending_events: VecDeque::new(),
        }
    }

    /// Adapter counters for diagnostics.
    #[must_use]
    pub const fn stats(&self) -> crate::resident::ResidentStats {
        self.adapter.stats()
    }
}

impl TextureDriver<TextureKey> for VulkanResidentDriver {
    fn pass(
        &mut self,
        ledger: &mut TextureLedger<TextureKey>,
        boundary: Boundary,
        events: Vec<LedgerEvent<TextureKey>>,
    ) -> PassOutcome {
        self.pending_events
            .extend(events.into_iter().filter_map(|event| match event {
                LedgerEvent::Prepared { key, revision } => {
                    Some(ResidentEvent::Prepared { key, revision })
                }
                LedgerEvent::UploadFailed {
                    key,
                    revision,
                    reason,
                } => Some(ResidentEvent::UploadFailed {
                    key,
                    revision,
                    reason,
                }),
                LedgerEvent::KeyRetired { key } => Some(ResidentEvent::KeyRetired { key }),
                LedgerEvent::GpuFailure { message } => Some(ResidentEvent::GpuFailure { message }),
                // The presenter reports this compatibility-specific failure before the driver runs.
                LedgerEvent::CompatUploadFailed { .. } => None,
            }));
        let boundary = match boundary {
            Boundary::Pass => ResidentBoundary::Ordinary,
            Boundary::Frame => ResidentBoundary::Frame,
            Boundary::Ending => ResidentBoundary::Ending,
        };
        let outcome = self.adapter.pass(
            &mut VulkanLedgerOps {
                ledger,
                events: &mut self.pending_events,
            },
            boundary,
        );
        PassOutcome { more: outcome.more }
    }
}

#[cfg(test)]
mod tests {
    use ck_gpu_vulkan::TextureDriver;
    use ck_resources::TextureKey;

    use super::VulkanResidentDriver;

    #[test]
    fn vulkan_driver_implements_the_presenter_texture_seam() {
        fn assert_driver<D: TextureDriver<TextureKey>>() {}
        assert_driver::<VulkanResidentDriver>();
    }
}
