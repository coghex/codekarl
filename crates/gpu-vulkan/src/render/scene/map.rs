//! The scene presenter's map steps (milestone 6, slice 1): the thin side of the shared [`MapDriver`](crate::render::map). This thread
//! keeps what only it knows: the surface-format rule (the enlarged map is drawn by the quad's pipeline, so an unsupported format
//! skips it with that rule's single notice) and its stop rules.

use super::Core;
use crate::map::VulkanMap;
use crate::render::map::{MapContext, MapPrep};
use crate::render::{Fatal, Prepared};

impl Core<'_> {
    /// Prepares the map of a frame that is going to be drawn. A scene without a map, or a surface whose format the quad's pipeline
    /// is not exact on, has no map pass and keeps the resident objects as they are.
    pub(in crate::render) fn prepare_map(
        &mut self,
        map: Option<&VulkanMap>,
    ) -> Result<MapPrep, Fatal> {
        let Some(map) = map else {
            self.map.not_attempted();
            return Ok(MapPrep::Nothing);
        };
        if !self.quad_supported {
            self.skip_notice();
            self.map.not_attempted();
            return Ok(MapPrep::Nothing);
        }
        let (may_dispose, healthy) = (self.may_dispose(false), self.healthy());
        self.map.prepare(
            MapContext {
                gpu: &mut self.gpu,
                ledger: &mut self.ledger,
                hooks: &mut self.hooks,
                observer: self.observer.as_deref(),
                may_dispose,
                healthy,
            },
            map,
        )
    }

    /// The frame was submitted as `number`.
    pub(in crate::render) fn map_submitted(&mut self, number: u64, prepared: &Prepared) {
        self.map
            .submitted(number, prepared, self.observer.as_deref());
    }

    /// Destroys what the ledger's proofs allow and the state permits (`stopping` is the stop state the caller read).
    pub(in crate::render) fn reap_map(&mut self, stopping: bool) {
        let may_dispose = self.may_dispose(stopping);
        self.map.reap(
            &mut self.gpu,
            &self.ledger,
            self.observer.as_deref(),
            may_dispose,
        );
    }
}
