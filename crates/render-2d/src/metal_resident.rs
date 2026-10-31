//! Thin binding of the resident adapter to CK-114's Metal texture driver seam.

use std::{collections::VecDeque, sync::Arc};

use ck_gpu_metal::{
    Boundary, LedgerEvent, OfferRefusal, PassOutcome, TextureDriver, TextureLedger,
};
use ck_resources::{PixelRevision, ResourceService, TextureKey, UploadRequest};

use crate::resident::{
    LedgerOps, ResidentAdapter, ResidentBoundary, ResidentEvent, ResidentRefusal,
};

struct RevisionPixels(Arc<PixelRevision>);

impl ck_gpu_metal::PixelSource for RevisionPixels {
    fn pixels(&self) -> &[u8] {
        self.0.pixels()
    }
}

/// Metal ledger implementation of the backend-neutral ledger operations.
struct MetalLedgerOps<'a> {
    ledger: &'a mut TextureLedger<TextureKey>,
    events: &'a mut VecDeque<ResidentEvent>,
}

impl LedgerOps for MetalLedgerOps<'_> {
    fn room(&self) -> usize {
        self.ledger.room()
    }

    fn extra_room(&self) -> usize {
        self.ledger.extra_room()
    }

    fn offer(&mut self, request: UploadRequest) -> Result<(), ResidentRefusal> {
        let source: Arc<dyn ck_gpu_metal::PixelSource> =
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

/// Metal's `TextureDriver` backed by the backend-neutral resident adapter.
pub struct MetalResidentDriver {
    adapter: ResidentAdapter,
    pending_events: VecDeque<ResidentEvent>,
}

impl MetalResidentDriver {
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

impl TextureDriver<TextureKey> for MetalResidentDriver {
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
                // The presenter takes this observation before any driver runs (its own `Failed` report); the driver's
                // `UploadFailed` of the same failure is what the service's status shows.
                LedgerEvent::CompatUploadFailed { .. } => None,
            }));
        let boundary = match boundary {
            Boundary::Pass => ResidentBoundary::Ordinary,
            Boundary::Frame => ResidentBoundary::Frame,
            Boundary::Ending => ResidentBoundary::Ending,
        };
        let outcome = self.adapter.pass(
            &mut MetalLedgerOps {
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
    use ck_gpu_metal::TextureDriver;
    use ck_resources::TextureKey;

    use super::MetalResidentDriver;

    #[test]
    fn metal_driver_implements_the_presenter_texture_seam() {
        fn assert_driver<D: TextureDriver<TextureKey>>() {}
        assert_driver::<MetalResidentDriver>();
    }
}
