//! The present-lifetime ledger (design §8): pure bookkeeping, with no Vulkan call in it.
//!
//! The render thread performs the Vulkan operations and tells the ledger what happened; the ledger knows
//! which semaphore has a pending signal, which fence will prove each operation finished, which swap chain
//! each proof belongs to, and what may be reused or destroyed. Objects are named by their raw handles, so
//! every transition is tested here without a device.
//!
//! The transitions, each a method:
//!
//! - **an acquire that succeeds** ([`Ledger::acquired`]) records the image as acquired and its acquire
//!   semaphore as pending a signal;
//! - **a submit** ([`Ledger::submitted`]) records the submit fence as proof for the acquire semaphore and for
//!   the image's rendering, and the render-finished semaphore as pending a signal;
//! - **a present** that returns success, `VK_SUBOPTIMAL_KHR`, `VK_ERROR_OUT_OF_DATE_KHR` or
//!   `VK_ERROR_SURFACE_LOST_KHR` queued its semaphore wait and its present fence ([`Ledger::presented`]
//!   records the fence as outstanding); one that fails with host or device out-of-memory queued nothing, so
//!   its present fence is never waited on and its render-finished semaphore is drained by a waiting submit
//!   before it is reused ([`Ledger::drain_render`]);
//! - **a frame abandoned after acquire** ([`Ledger::abandoned`]): the image is released and the acquire
//!   semaphore drained by a submit that waits on it;
//! - **a swap chain** is retired by a rebuild ([`Ledger::retire_current`]) or by a failed creation
//!   ([`Ledger::creation_failed`]), and destroyed only once nothing pending belongs to it
//!   ([`Ledger::retired_destroyable`]);
//! - **`VK_ERROR_DEVICE_LOST`** ([`Ledger::device_lost`]): nothing can be proven, so nothing is ever
//!   reported proven again.

/// A raw Vulkan handle.
pub(crate) type Raw = u64;
/// The ledger's name for a swap chain.
pub(crate) type ChainId = u64;

/// A render-finished semaphore and the present fence that goes with it. They return to the pool together,
/// only after the present fence has signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentSlot {
    pub(crate) semaphore: Raw,
    pub(crate) fence: Raw,
}

/// What one frame step takes from the pools. Taking is all-or-nothing: an empty pool skips the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) acquire: Raw,
    pub(crate) slot: PresentSlot,
    pub(crate) submit_fence: Raw,
    /// A spare fence for a drain submit; goes back unused when nothing needs draining.
    pub(crate) spare_fence: Raw,
}

/// What a signalled fence frees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Free {
    AcquireSemaphore(Raw),
    PresentSlot(PresentSlot),
    Fence(Raw),
}

/// What a pending fence proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Proof {
    /// The GPU finished a frame's commands (and so the acquire semaphore's wait).
    Submit,
    /// The presentation engine finished with the render-finished semaphore and the image.
    Present,
    /// A drain submit consumed an acquire semaphore's pending signal.
    AcquireDrain,
    /// A drain submit consumed a render-finished semaphore's pending signal.
    RenderDrain,
}

/// A scene frame's submit, whose proof is the texture ledger's latched `frame_proven(number)`, not a fence of this
/// ledger: the frame's fence stays inside the texture ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Numbered {
    number: u64,
    chain: Option<ChainId>,
    acquire: Raw,
}

#[derive(Debug)]
struct Pending {
    fence: Raw,
    chain: Option<ChainId>,
    proof: Proof,
    frees: Vec<Free>,
    /// The submit's number, for a `Proof::Submit`.
    submit: Option<u64>,
}

/// An image that was acquired and is neither submitted nor released yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Acquired {
    pub(crate) index: u32,
    pub(crate) chain: ChainId,
}

/// How a present call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentCall {
    /// `VK_SUCCESS`.
    Success,
    /// `VK_SUBOPTIMAL_KHR`: queued; the swap chain should be rebuilt.
    Suboptimal,
    /// `VK_ERROR_OUT_OF_DATE_KHR`: queued; the swap chain must be rebuilt.
    OutOfDate,
    /// `VK_ERROR_SURFACE_LOST_KHR`: queued; nothing more can be drawn.
    SurfaceLost,
    /// `VK_ERROR_OUT_OF_HOST_MEMORY` or `VK_ERROR_OUT_OF_DEVICE_MEMORY`: nothing was queued.
    OutOfMemory,
}

/// What the ledger concluded from a present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presented {
    /// The present fence is outstanding. `rebuild` asks for a new swap chain, `fatal` ends the drawing.
    Outstanding { rebuild: bool, fatal: bool },
    /// Nothing was queued: the render-finished semaphore must be drained, then the drawing ends.
    NeedsDrain,
}

/// A snapshot of the ledger, for the tests' observer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a snapshot of independent facts, each read on its own"
)]
pub struct LedgerSnapshot {
    /// Submit fences outstanding.
    pub pending_submits: usize,
    /// Present fences outstanding.
    pub pending_presents: usize,
    /// Drain submits outstanding (acquire and render-finished).
    pub pending_drains: usize,
    /// Acquire semaphores free in the pool.
    pub free_acquire: usize,
    /// Render-finished semaphore and present fence pairs free in the pool.
    pub free_slots: usize,
    /// Fences free in the pool.
    pub free_fences: usize,
    /// Whether a swap chain is current.
    pub has_current: bool,
    /// Whether a swap chain is retired and not yet destroyed.
    pub has_retired: bool,
    /// Whether an image is acquired and neither submitted nor released.
    pub image_acquired: bool,
    /// Whether the device was lost.
    pub lost: bool,
}

/// The ledger (see the module documentation).
#[derive(Debug)]
pub(crate) struct Ledger {
    acquire_free: Vec<Raw>,
    slot_free: Vec<PresentSlot>,
    fence_free: Vec<Raw>,
    pending: Vec<Pending>,
    acquired: Option<Acquired>,
    /// A slot whose present queued nothing: its render-finished semaphore still has a pending signal.
    undrained: Option<PresentSlot>,
    current: Option<ChainId>,
    retired: Option<ChainId>,
    next_chain: ChainId,
    lost: bool,
    /// How many frame submits were made: each is numbered from 1 in the order made.
    submits_made: u64,
    /// The highest submit number whose fence was seen signalled. Frames are submitted one at a time (the next
    /// starts after the last submit's proof), so every number up to it is proven. This is the ledger's own
    /// record: a fence handle is reset and reused after its proof, and says nothing about an earlier submit.
    submits_proven: u64,
    /// Scene frames submitted and not yet seen proven through the texture ledger, oldest first.
    numbered: Vec<Numbered>,
}

impl Ledger {
    /// A ledger over fixed pools.
    pub(crate) fn new(acquire: Vec<Raw>, slots: Vec<PresentSlot>, fences: Vec<Raw>) -> Self {
        Self {
            acquire_free: acquire,
            slot_free: slots,
            fence_free: fences,
            pending: Vec::new(),
            acquired: None,
            undrained: None,
            current: None,
            retired: None,
            next_chain: 1,
            lost: false,
            submits_made: 0,
            submits_proven: 0,
            numbered: Vec::new(),
        }
    }

    // ---- the pools ----

    /// Whether [`take_frame`](Self::take_frame) would give a frame: every pool has what a frame takes.
    pub(crate) fn can_take_frame(&self) -> bool {
        !self.acquire_free.is_empty() && !self.slot_free.is_empty() && self.fence_free.len() >= 2
    }

    /// Takes an acquire semaphore, a present slot and two fences, or nothing at all.
    pub(crate) fn take_frame(&mut self) -> Option<Frame> {
        if self.acquire_free.is_empty() || self.slot_free.is_empty() || self.fence_free.len() < 2 {
            return None;
        }
        let acquire = self.acquire_free.pop()?;
        let slot = self.slot_free.pop()?;
        let submit_fence = self.fence_free.pop()?;
        let spare_fence = self.fence_free.pop()?;
        Some(Frame {
            acquire,
            slot,
            submit_fence,
            spare_fence,
        })
    }

    /// Gives back everything of a frame that never started an operation (an acquire that failed or timed
    /// out signals nothing).
    pub(crate) fn return_unused(&mut self, frame: Frame) {
        self.acquire_free.push(frame.acquire);
        self.slot_free.push(frame.slot);
        self.fence_free.push(frame.submit_fence);
        self.fence_free.push(frame.spare_fence);
    }

    // ---- the transitions ----

    /// An acquire succeeded: the image is acquired and `frame.acquire` is pending a signal.
    pub(crate) fn acquired(&mut self, index: u32, chain: ChainId) {
        self.acquired = Some(Acquired { index, chain });
    }

    /// The image acquired and not yet submitted or released, if any.
    #[cfg(test)]
    pub(crate) fn acquired_image(&self) -> Option<Acquired> {
        self.acquired
    }

    /// The submit was queued. Its fence proves the acquire semaphore's wait and the image's rendering; the
    /// render-finished semaphore has a pending signal (until a present waits on it).
    ///
    /// Returns the submit's number, which [`Ledger::submit_proven`] later answers for.
    pub(crate) fn submitted(&mut self, frame: &Frame) -> u64 {
        let chain = self.acquired.map(|acquired| acquired.chain);
        self.submits_made += 1;
        self.pending.push(Pending {
            fence: frame.submit_fence,
            chain,
            proof: Proof::Submit,
            frees: vec![
                Free::AcquireSemaphore(frame.acquire),
                Free::Fence(frame.submit_fence),
            ],
            submit: Some(self.submits_made),
        });
        // The spare fence is not needed unless a drain follows.
        self.submits_made
    }

    /// A scene frame was submitted through the texture ledger as submission `number`. The submit fence of `frame` was
    /// never used (the texture ledger's own fence proves the frame), so it returns to the pool at once; the acquire
    /// semaphore returns when [`Ledger::reap_numbered`] is told `number` is proven.
    pub(crate) fn submitted_numbered(&mut self, frame: &Frame, number: u64) {
        let chain = self.acquired.map(|acquired| acquired.chain);
        self.fence_free.push(frame.submit_fence);
        self.numbered.push(Numbered {
            number,
            chain,
            acquire: frame.acquire,
        });
    }

    /// Frees what every numbered submit for which `proven` answers true was holding (its acquire semaphore), in order.
    /// `proven` is the texture ledger's `frame_proven`: the one authoritative record, true for good once seen.
    /// Returns how many were freed.
    pub(crate) fn reap_numbered(&mut self, proven: &dyn Fn(u64) -> bool) -> usize {
        let before = self.numbered.len();
        let mut kept = Vec::with_capacity(before);
        for item in std::mem::take(&mut self.numbered) {
            if proven(item.number) {
                self.acquire_free.push(item.acquire);
                self.submits_proven = self.submits_proven.max(item.number);
            } else {
                kept.push(item);
            }
        }
        self.numbered = kept;
        before - self.numbered.len()
    }

    /// The number of the last frame submit made (0 before the first).
    #[cfg(test)]
    pub(crate) fn last_submit(&self) -> u64 {
        self.submits_made
    }

    /// The highest submit number proven finished (0 before the first proof).
    pub(crate) fn proven_through(&self) -> u64 {
        self.submits_proven
    }

    /// Whether submit `number` is proven finished: its fence was seen signalled. Decided from this record, never
    /// from a fence handle, which may since have been reset and used for a later submit.
    #[cfg(test)]
    pub(crate) fn submit_proven(&self, number: u64) -> bool {
        number <= self.submits_proven
    }

    /// The present returned `call`.
    pub(crate) fn presented(&mut self, frame: &Frame, call: PresentCall) -> Presented {
        let chain = self.acquired.take().map(|acquired| acquired.chain);
        match call {
            PresentCall::Success
            | PresentCall::Suboptimal
            | PresentCall::OutOfDate
            | PresentCall::SurfaceLost => {
                // The spare fence was not needed for a drain.
                self.fence_free.push(frame.spare_fence);
                self.pending.push(Pending {
                    fence: frame.slot.fence,
                    chain,
                    proof: Proof::Present,
                    frees: vec![Free::PresentSlot(frame.slot)],
                    submit: None,
                });
                Presented::Outstanding {
                    rebuild: matches!(call, PresentCall::Suboptimal | PresentCall::OutOfDate),
                    fatal: call == PresentCall::SurfaceLost,
                }
            }
            PresentCall::OutOfMemory => {
                self.undrained = Some(frame.slot);
                Presented::NeedsDrain
            }
        }
    }

    /// The drain submit that waits on the undrained render-finished semaphore was queued with `fence` (the
    /// frame's spare fence).
    pub(crate) fn drain_render(&mut self, fence: Raw) {
        let Some(slot) = self.undrained.take() else {
            return;
        };
        self.pending.push(Pending {
            fence,
            chain: None,
            proof: Proof::RenderDrain,
            frees: vec![Free::PresentSlot(slot), Free::Fence(fence)],
            submit: None,
        });
    }

    /// A frame abandoned after acquire: the image is released, and the drain submit queued with
    /// `frame.spare_fence` consumes the acquire semaphore's pending signal. The present slot and the submit
    /// fence were never used and go back at once.
    pub(crate) fn abandoned(&mut self, frame: &Frame) {
        let chain = self.acquired.take().map(|acquired| acquired.chain);
        self.pending.push(Pending {
            fence: frame.spare_fence,
            chain,
            proof: Proof::AcquireDrain,
            frees: vec![
                Free::AcquireSemaphore(frame.acquire),
                Free::Fence(frame.spare_fence),
            ],
            submit: None,
        });
        self.slot_free.push(frame.slot);
        self.fence_free.push(frame.submit_fence);
    }

    /// A fence was seen signalled: whatever it proved is freed. Returns what it proved and for which swap
    /// chain.
    pub(crate) fn fence_signalled(&mut self, fence: Raw) -> Option<(Proof, Option<ChainId>)> {
        let position = self
            .pending
            .iter()
            .position(|pending| pending.fence == fence)?;
        let done = self.pending.remove(position);
        if let Some(number) = done.submit {
            self.submits_proven = self.submits_proven.max(number);
        }
        for free in done.frees {
            match free {
                Free::AcquireSemaphore(semaphore) => self.acquire_free.push(semaphore),
                Free::PresentSlot(slot) => self.slot_free.push(slot),
                Free::Fence(fence) => self.fence_free.push(fence),
            }
        }
        Some((done.proof, done.chain))
    }

    /// The fences the ledger waits on, oldest first, with what each proves.
    pub(crate) fn pending(&self) -> Vec<(Raw, Proof)> {
        self.pending
            .iter()
            .map(|pending| (pending.fence, pending.proof))
            .collect()
    }

    /// The fences the ledger waits on, oldest first.
    #[cfg(test)]
    pub(crate) fn pending_fences(&self) -> Vec<Raw> {
        self.pending.iter().map(|pending| pending.fence).collect()
    }

    /// Whether a frame's commands are still being proven (one frame in flight).
    pub(crate) fn submit_in_flight(&self) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.proof == Proof::Submit)
            || !self.numbered.is_empty()
    }

    /// Whether anything is pending a proof (so the thread must keep looking at fences).
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty() || !self.numbered.is_empty()
    }

    // ---- the swap chains ----

    /// Names the next swap chain (before it is created).
    pub(crate) fn reserve_chain(&mut self) -> ChainId {
        let id = self.next_chain;
        self.next_chain += 1;
        id
    }

    /// The swap chain `id` was created and became current.
    pub(crate) fn chain_created(&mut self, id: ChainId) {
        self.current = Some(id);
    }

    /// The current swap chain, if any.
    #[cfg(test)]
    pub(crate) fn current(&self) -> Option<ChainId> {
        self.current
    }

    /// The retired swap chain, if any.
    pub(crate) fn retired(&self) -> Option<ChainId> {
        self.retired
    }

    /// Whether a rebuild may start: at most one swap chain is retired.
    #[cfg(test)]
    pub(crate) fn can_retire(&self) -> bool {
        self.retired.is_none()
    }

    /// A rebuild retires the current swap chain (the new one is created with it as `oldSwapchain`).
    pub(crate) fn retire_current(&mut self) -> Option<ChainId> {
        if self.retired.is_some() {
            return None;
        }
        let old = self.current.take()?;
        self.retired = Some(old);
        Some(old)
    }

    /// Creating the new swap chain failed: the old one, passed as `oldSwapchain`, is retired anyway and can
    /// no longer be acquired from; nothing is current, rendering pauses, and the rebuild is retried once the
    /// retired swap chain is destroyed. (Same effect as a retirement without a successor.)
    pub(crate) fn creation_failed(&mut self) {
        // Already retired by `retire_current` before the attempt; nothing is current.
        self.current = None;
    }

    /// Whether any proof still pending belongs to `chain`.
    pub(crate) fn chain_busy(&self, chain: ChainId) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.chain == Some(chain))
            || self
                .numbered
                .iter()
                .any(|numbered| numbered.chain == Some(chain))
            || self
                .acquired
                .is_some_and(|acquired| acquired.chain == chain)
    }

    /// The retired swap chain, if every present fence and submit that used it has signalled.
    ///
    /// Never after the device was lost: nothing is destroyed then, a retired swap chain included, even if
    /// its fences signal.
    pub(crate) fn retired_destroyable(&self) -> Option<ChainId> {
        if self.lost {
            return None;
        }
        self.retired.filter(|chain| !self.chain_busy(*chain))
    }

    /// The retired swap chain was destroyed.
    pub(crate) fn retired_destroyed(&mut self) {
        self.retired = None;
    }

    // ---- loss and proof ----

    /// `VK_ERROR_DEVICE_LOST`: nothing can be proven again.
    pub(crate) fn device_lost(&mut self) {
        self.lost = true;
    }

    /// Whether the device was lost.
    pub(crate) fn is_lost(&self) -> bool {
        self.lost
    }

    /// Whether every operation is proven finished (the teardown may destroy): nothing pending, no image
    /// acquired, no semaphore waiting for a drain, and the device not lost.
    pub(crate) fn proven(&self) -> bool {
        !self.lost
            && self.pending.is_empty()
            && self.numbered.is_empty()
            && self.acquired.is_none()
            && self.undrained.is_none()
    }

    /// What is outstanding, in words (an `Unproven` reason).
    pub(crate) fn describe_outstanding(&self) -> String {
        if self.lost {
            return "the device was lost".to_owned();
        }
        let count = |wanted: Proof| {
            self.pending
                .iter()
                .filter(|pending| pending.proof == wanted)
                .count()
        };
        format!(
            "{} submit fence(s), {} present fence(s) and {} drain(s) outstanding{}",
            count(Proof::Submit) + self.numbered.len(),
            count(Proof::Present),
            count(Proof::AcquireDrain) + count(Proof::RenderDrain),
            if self.acquired.is_some() {
                ", an image acquired and not released"
            } else if self.undrained.is_some() {
                ", a render-finished semaphore not drained"
            } else {
                ""
            }
        )
    }

    /// A snapshot for the observer.
    pub(crate) fn snapshot(&self) -> LedgerSnapshot {
        let count = |wanted: Proof| {
            self.pending
                .iter()
                .filter(|pending| pending.proof == wanted)
                .count()
        };
        LedgerSnapshot {
            pending_submits: count(Proof::Submit) + self.numbered.len(),
            pending_presents: count(Proof::Present),
            pending_drains: count(Proof::AcquireDrain) + count(Proof::RenderDrain),
            free_acquire: self.acquire_free.len(),
            free_slots: self.slot_free.len(),
            free_fences: self.fence_free.len(),
            has_current: self.current.is_some(),
            has_retired: self.retired.is_some(),
            image_acquired: self.acquired.is_some(),
            lost: self.lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(ledger: &mut Ledger) -> ChainId {
        let id = ledger.reserve_chain();
        ledger.chain_created(id);
        id
    }

    /// Two images' worth: 4 acquire semaphores, 4 slots, 11 fences (1 + 4 + 2 spare ... enough).
    fn ledger() -> Ledger {
        Ledger::new(
            (1..=4).collect(),
            (0..4)
                .map(|n| PresentSlot {
                    semaphore: 100 + n,
                    fence: 200 + n,
                })
                .collect(),
            (300..310).collect(),
        )
    }

    /// A scene frame submitted as number `number`: the acquire is taken, the image acquired, the submit numbered.
    fn numbered_frame(ledger: &mut Ledger, chain: ChainId, number: u64) -> Frame {
        let frame = ledger.take_frame().expect("the pools have room");
        ledger.acquired(0, chain);
        ledger.submitted_numbered(&frame, number);
        frame
    }

    #[test]
    fn a_numbered_submit_returns_its_fence_at_once_and_its_acquire_semaphore_when_its_number_is_proven()
     {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let before = ledger.snapshot();
        let frame = numbered_frame(&mut ledger, chain, 1);
        let during = ledger.snapshot();
        assert_eq!(
            during.free_fences,
            before.free_fences - 1,
            "the spare fence is out, the submit fence came back at once"
        );
        assert_eq!(during.free_acquire, before.free_acquire - 1);
        assert_eq!(
            during.pending_submits, 1,
            "a numbered submit is a pending submit"
        );
        assert!(ledger.submit_in_flight() && ledger.has_pending() && !ledger.proven());
        assert_eq!(
            ledger.reap_numbered(&|_| false),
            0,
            "not proven: nothing is freed"
        );
        assert_eq!(ledger.snapshot().free_acquire, before.free_acquire - 1);
        assert_eq!(ledger.reap_numbered(&|number| number == 1), 1);
        assert_eq!(ledger.snapshot().free_acquire, before.free_acquire);
        assert!(!ledger.submit_in_flight());
        assert_eq!(ledger.proven_through(), 1);
        // The present half of the frame is the ledger's own, unchanged.
        ledger.presented(&frame, PresentCall::Success);
        assert!(ledger.fence_signalled(frame.slot.fence).is_some());
        assert!(ledger.proven());
    }

    #[test]
    fn a_retired_swap_chain_is_not_destroyable_while_a_numbered_submit_on_it_is_unproven() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let frame = numbered_frame(&mut ledger, chain, 1);
        ledger.presented(&frame, PresentCall::Success);
        ledger.fence_signalled(frame.slot.fence);
        ledger
            .retire_current()
            .expect("a rebuild retires the current chain");
        assert_eq!(
            ledger.retired_destroyable(),
            None,
            "the frame's commands may still use the swap chain's framebuffers"
        );
        ledger.reap_numbered(&|number| number == 1);
        assert_eq!(ledger.retired_destroyable(), Some(chain));
    }

    /// The texture ledger's record is latched: a number stays proven after its fence is reset and used for a later
    /// frame. The present ledger asks the record, so it reaps in any order and any number of looks later.
    #[test]
    fn a_proof_that_stays_true_reaps_a_frame_after_its_fence_was_reused() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let first = numbered_frame(&mut ledger, chain, 1);
        ledger.presented(&first, PresentCall::Success);
        // The record says 1 is proven now, but the present ledger does not look until frame 3 is submitted.
        let second = numbered_frame(&mut ledger, chain, 3);
        ledger.presented(&second, PresentCall::Success);
        let latched = |number: u64| number <= 1;
        assert_eq!(
            ledger.reap_numbered(&latched),
            1,
            "frame 1 is returned, 3 is not"
        );
        assert!(ledger.submit_in_flight(), "frame 3 is still unproven");
        assert_eq!(ledger.reap_numbered(&|number| number <= 3), 1);
        assert!(!ledger.submit_in_flight());
    }

    /// Negative control: reading the fence's *current* status after the fence was reset and reused for frame n + 2
    /// reports frame n unproven for good, so its acquire semaphore never returns and its slot stays stuck.
    #[test]
    fn negative_control_the_current_fence_status_after_reuse_never_proves_the_old_frame() {
        // The model: one fence object, signalled by frame 1, reset for frame 3, which has not signalled.
        struct Fence {
            signalled: bool,
            last_submission: u64,
        }
        let mut fence = Fence {
            signalled: false,
            last_submission: 1,
        };
        fence.signalled = true; // frame 1 finished
        fence.signalled = false; // reset for frame 3
        fence.last_submission = 3;
        let by_current_status = |_number: u64| fence.signalled;
        let by_latched_record = |number: u64| number <= 1;
        let mut stuck = ledger();
        let chain = created(&mut stuck);
        numbered_frame(&mut stuck, chain, 1);
        assert_eq!(
            stuck.reap_numbered(&by_current_status),
            0,
            "the mutant never proves frame 1"
        );
        assert!(
            stuck.submit_in_flight(),
            "and its acquire semaphore is stuck"
        );
        assert_eq!(fence.last_submission, 3);
        let mut whole = ledger();
        let chain = created(&mut whole);
        numbered_frame(&mut whole, chain, 1);
        assert_eq!(
            whole.reap_numbered(&by_latched_record),
            1,
            "the latched record proves it"
        );
    }

    /// One whole frame up to and including its present, returning the frame.
    fn frame_through_present(
        ledger: &mut Ledger,
        chain: ChainId,
        call: PresentCall,
    ) -> (Frame, Presented) {
        let frame = ledger.take_frame().expect("the pools have room");
        ledger.acquired(0, chain);
        ledger.submitted(&frame);
        let outcome = ledger.presented(&frame, call);
        (frame, outcome)
    }

    #[test]
    fn an_acquire_records_the_image_and_a_submit_records_the_proof_for_the_semaphore() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let frame = ledger.take_frame().expect("room");
        ledger.acquired(2, chain);
        assert_eq!(ledger.acquired_image(), Some(Acquired { index: 2, chain }));
        assert!(!ledger.proven(), "an acquired image is unproven");
        ledger.submitted(&frame);
        assert!(
            !ledger.acquire_free.contains(&frame.acquire),
            "the acquire semaphore is not reusable before the submit fence that waited on it"
        );
        assert!(ledger.submit_in_flight());
        assert_eq!(ledger.pending_fences(), vec![frame.submit_fence]);
        assert_eq!(
            ledger
                .fence_signalled(frame.submit_fence)
                .map(|(proof, _)| proof),
            Some(Proof::Submit)
        );
        assert!(
            ledger.acquire_free.contains(&frame.acquire),
            "the acquire semaphore returned after its submit fence"
        );
        assert!(ledger.fence_free.contains(&frame.submit_fence));
    }

    #[test]
    fn a_present_that_queued_keeps_the_slot_until_its_present_fence_signals() {
        for call in [
            PresentCall::Success,
            PresentCall::Suboptimal,
            PresentCall::OutOfDate,
            PresentCall::SurfaceLost,
        ] {
            let mut ledger = ledger();
            let chain = created(&mut ledger);
            let (frame, outcome) = frame_through_present(&mut ledger, chain, call);
            assert_eq!(
                outcome,
                Presented::Outstanding {
                    rebuild: matches!(call, PresentCall::Suboptimal | PresentCall::OutOfDate),
                    fatal: call == PresentCall::SurfaceLost,
                },
                "{call:?}"
            );
            assert!(
                !ledger.slot_free.contains(&frame.slot),
                "{call:?}: the slot is not reusable yet"
            );
            assert!(
                ledger.pending_fences().contains(&frame.slot.fence),
                "{call:?}: the present fence is outstanding"
            );
            assert!(
                ledger.chain_busy(chain),
                "{call:?}: the swap chain waits for its present fence"
            );
            ledger.fence_signalled(frame.submit_fence);
            assert!(
                ledger.chain_busy(chain),
                "{call:?}: the submit alone does not release the chain"
            );
            assert_eq!(
                ledger
                    .fence_signalled(frame.slot.fence)
                    .map(|(proof, _)| proof),
                Some(Proof::Present)
            );
            assert!(
                ledger.slot_free.contains(&frame.slot),
                "{call:?}: the slot returned after its fence"
            );
            assert!(!ledger.chain_busy(chain));
            assert!(ledger.proven(), "{call:?}");
        }
    }

    #[test]
    fn a_present_that_queued_nothing_is_drained_by_a_submit_before_the_slot_is_reused() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let (frame, outcome) = frame_through_present(&mut ledger, chain, PresentCall::OutOfMemory);
        assert_eq!(outcome, Presented::NeedsDrain);
        assert!(
            !ledger.pending_fences().contains(&frame.slot.fence),
            "its present fence was never submitted, so it is never waited on"
        );
        assert!(
            !ledger.proven(),
            "a semaphore with a pending signal is unproven"
        );
        assert!(!ledger.slot_free.contains(&frame.slot));
        ledger.drain_render(frame.spare_fence);
        assert!(
            !ledger.slot_free.contains(&frame.slot),
            "still not reusable before the drain's fence"
        );
        ledger.fence_signalled(frame.submit_fence);
        assert_eq!(
            ledger
                .fence_signalled(frame.spare_fence)
                .map(|(proof, _)| proof),
            Some(Proof::RenderDrain)
        );
        assert!(ledger.slot_free.contains(&frame.slot));
        assert!(ledger.proven());
    }

    #[test]
    fn a_frame_abandoned_after_acquire_drains_its_acquire_semaphore_and_returns_the_rest() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let frame = ledger.take_frame().expect("room");
        ledger.acquired(1, chain);
        ledger.abandoned(&frame);
        assert!(ledger.acquired_image().is_none(), "the image was released");
        assert!(
            ledger.slot_free.contains(&frame.slot),
            "the unused slot returned at once"
        );
        assert!(ledger.fence_free.contains(&frame.submit_fence));
        assert!(
            !ledger.acquire_free.contains(&frame.acquire),
            "its pending signal must be drained first"
        );
        assert!(
            ledger.chain_busy(chain),
            "the drain belongs to the swap chain"
        );
        assert_eq!(
            ledger
                .fence_signalled(frame.spare_fence)
                .map(|(proof, _)| proof),
            Some(Proof::AcquireDrain)
        );
        assert!(ledger.acquire_free.contains(&frame.acquire));
        assert!(ledger.proven());
    }

    #[test]
    fn an_empty_pool_skips_the_frame_and_takes_nothing() {
        let mut ledger = Ledger::new(
            vec![1],
            vec![PresentSlot {
                semaphore: 2,
                fence: 3,
            }],
            vec![4],
        );
        assert_eq!(
            ledger.take_frame(),
            None,
            "one fence is too few: nothing may be taken"
        );
        assert_eq!(ledger.acquire_free.len(), 1);
        assert_eq!(ledger.slot_free.len(), 1);
        assert_eq!(ledger.fence_free.len(), 1);
        let mut full = ledger_with_one_frame();
        let first = full.take_frame().expect("one frame");
        assert_eq!(
            full.take_frame(),
            None,
            "the pools are fixed: the next frame waits"
        );
        full.return_unused(first);
        assert!(full.take_frame().is_some());
    }

    fn ledger_with_one_frame() -> Ledger {
        Ledger::new(
            vec![1],
            vec![PresentSlot {
                semaphore: 2,
                fence: 3,
            }],
            vec![4, 5],
        )
    }

    #[test]
    fn at_most_one_swap_chain_is_retired_and_it_is_destroyable_only_after_its_fences() {
        let mut ledger = ledger();
        let first = created(&mut ledger);
        let (frame, _) = frame_through_present(&mut ledger, first, PresentCall::OutOfDate);
        assert_eq!(ledger.retire_current(), Some(first));
        assert!(
            !ledger.can_retire(),
            "a second rebuild pauses while one chain is retired"
        );
        assert_eq!(ledger.retire_current(), None);
        assert_eq!(
            ledger.retired_destroyable(),
            None,
            "its present fence is outstanding"
        );
        let second = created(&mut ledger);
        assert_ne!(first, second);
        ledger.fence_signalled(frame.submit_fence);
        assert_eq!(
            ledger.retired_destroyable(),
            None,
            "the present fence has not signalled"
        );
        ledger.fence_signalled(frame.slot.fence);
        assert_eq!(ledger.retired_destroyable(), Some(first));
        ledger.retired_destroyed();
        assert!(ledger.can_retire());
        assert_eq!(ledger.current(), Some(second));
    }

    #[test]
    fn a_failed_creation_retires_the_old_chain_anyway_and_pauses_rendering() {
        let mut ledger = ledger();
        let first = created(&mut ledger);
        assert_eq!(ledger.retire_current(), Some(first));
        ledger.creation_failed();
        assert_eq!(
            ledger.current(),
            None,
            "nothing is current: rendering pauses"
        );
        assert_eq!(
            ledger.retired(),
            Some(first),
            "the old chain was retired by the attempt"
        );
        assert_eq!(ledger.retired_destroyable(), Some(first));
        ledger.retired_destroyed();
        assert!(
            ledger.can_retire(),
            "the rebuild is retried once the retired chain is destroyed"
        );
    }

    #[test]
    fn a_retired_swap_chain_is_never_destroyable_after_device_loss() {
        let mut ledger = ledger();
        let first = created(&mut ledger);
        let (frame, _) = frame_through_present(&mut ledger, first, PresentCall::Success);
        assert_eq!(ledger.retire_current(), Some(first));
        ledger.fence_signalled(frame.submit_fence);
        ledger.fence_signalled(frame.slot.fence);
        assert_eq!(
            ledger.retired_destroyable(),
            Some(first),
            "healthy: destroyable once proven"
        );
        ledger.device_lost();
        assert_eq!(
            ledger.retired_destroyable(),
            None,
            "lost: nothing is destroyed any more"
        );
    }

    #[test]
    fn device_loss_means_nothing_is_ever_proven_again() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let (frame, _) = frame_through_present(&mut ledger, chain, PresentCall::Success);
        ledger.device_lost();
        ledger.fence_signalled(frame.submit_fence);
        ledger.fence_signalled(frame.slot.fence);
        assert!(ledger.is_lost());
        assert!(
            !ledger.proven(),
            "every object is kept: the cleanup is a failure"
        );
        assert!(ledger.describe_outstanding().contains("lost"));
    }

    #[test]
    fn the_snapshot_counts_what_is_outstanding() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let (frame, _) = frame_through_present(&mut ledger, chain, PresentCall::Success);
        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.pending_submits, 1);
        assert_eq!(snapshot.pending_presents, 1);
        assert_eq!(snapshot.free_slots, 3);
        assert!(snapshot.has_current && !snapshot.has_retired && !snapshot.lost);
        ledger.fence_signalled(frame.submit_fence);
        ledger.fence_signalled(frame.slot.fence);
        assert_eq!(
            ledger.snapshot().pending_submits + ledger.snapshot().pending_presents,
            0
        );
        assert_eq!(ledger.snapshot().free_fences, 10, "every fence is back");
    }
    #[test]
    fn a_submits_proof_is_recorded_before_its_fence_is_reset_and_reused() {
        // One acquire semaphore, one slot and exactly two fences: the next frame must reuse the fence of the
        // last.
        let mut ledger = Ledger::new(
            vec![1],
            vec![PresentSlot {
                semaphore: 2,
                fence: 3,
            }],
            vec![300, 301],
        );
        let chain = created(&mut ledger);
        assert_eq!((ledger.last_submit(), ledger.proven_through()), (0, 0));
        let first = ledger.take_frame().expect("room");
        ledger.acquired(0, chain);
        let number = ledger.submitted(&first);
        assert_eq!(number, 1);
        assert!(
            !ledger.submit_proven(1),
            "not proven before its fence signals"
        );
        ledger.presented(&first, PresentCall::Success);
        assert_eq!(
            ledger
                .fence_signalled(first.submit_fence)
                .map(|(proof, _)| proof),
            Some(Proof::Submit)
        );
        assert!(ledger.submit_proven(1), "proven by the signalled fence");
        ledger.fence_signalled(first.slot.fence);
        // The fence is back in the pool and the next frame uses it again, unsignalled.
        let second = ledger.take_frame().expect("room again");
        assert_eq!(
            second.submit_fence, first.submit_fence,
            "the fence handle is reused"
        );
        ledger.acquired(1, chain);
        assert_eq!(ledger.submitted(&second), 2);
        assert!(
            ledger.submit_proven(1),
            "the first submit stays proven while the reused fence is pending for the second"
        );
        assert!(
            !ledger.submit_proven(2),
            "the second is not proven by the first's signal"
        );
        assert_eq!(ledger.proven_through(), 1);
        ledger.presented(&second, PresentCall::Success);
        ledger.fence_signalled(second.submit_fence);
        assert!(ledger.submit_proven(2));
        assert_eq!((ledger.last_submit(), ledger.proven_through()), (2, 2));
    }

    #[test]
    fn drains_and_present_fences_are_not_submit_proofs() {
        let mut ledger = ledger();
        let chain = created(&mut ledger);
        let frame = ledger.take_frame().expect("room");
        ledger.acquired(0, chain);
        ledger.abandoned(&frame);
        ledger.fence_signalled(frame.spare_fence);
        assert_eq!(
            (ledger.last_submit(), ledger.proven_through()),
            (0, 0),
            "an abandoned frame made no submit"
        );
        let (frame, _) = frame_through_present(&mut ledger, chain, PresentCall::Success);
        ledger.fence_signalled(frame.slot.fence);
        assert!(
            !ledger.submit_proven(1),
            "a present fence does not prove the submit"
        );
        ledger.fence_signalled(frame.submit_fence);
        assert!(ledger.submit_proven(1));
    }

    /// Test 7, signal between observations, with both ledgers real (the texture ledger, a real device and real fences) and
    /// the present ledger around them: frame 1 is held by a `VkEvent` and the present ledger finds it unproven; the event is
    /// set; the texture ledger's pump latches frame 1, and a later `begin_frame` takes frame 1's fence, resets it and
    /// submits frame 3 on it (held by a second event) **before** the present ledger looks again; then the present ledger
    /// sees `frame_proven(1)` true, returns frame 1's acquire semaphore (and frame 2's) and `frame_proven(3)` stays false
    /// until its own signal.
    ///
    /// Negative control: the fence frame 1 used now belongs to frame 3 and is unsignalled, so a present ledger that read the
    /// fence's *current* status would still find frame 1 unproven (`negative_control_the_current_fence_status_after_reuse_never_proves_the_old_frame`
    /// models it; here the real state shows the premise: frame 3 is unproven while frame 1 is proven).
    #[test]
    #[expect(
        clippy::too_many_lines,
        clippy::multiple_unsafe_ops_per_block,
        reason = "one real sequence of two ledgers, a device, two events and their cleanup, read best together"
    )]
    fn a_proof_latched_before_the_fence_was_reused_returns_the_old_frames_semaphore() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use crate::revisions::{LedgerLimits, TextureLedger};
        use crate::textures::{TestHooks, TextureLimits, TextureWakes};
        use crate::{
            InstanceOptions, TestFeatures, VulkanContext, create_instance_with, load_loader,
        };
        use ash::vk;

        let loader = load_loader().expect("the Vulkan loader loads");
        let instance = create_instance_with(
            &loader,
            &[],
            InstanceOptions {
                validation: true,
                synchronization: true,
            },
        )
        .expect("a validated instance");
        let log = instance.validation_log();
        let context = VulkanContext::new_for_tests(
            &instance,
            None,
            TestFeatures {
                events: true,
                ..TestFeatures::default()
            },
        )
        .expect("a context with host events");
        let mut queue = context.take_queue().expect("the queue");
        let texture_ledger = TextureLedger::<u32>::start_with_test_hooks(
            Arc::clone(&context),
            TextureLimits::default(),
            LedgerLimits::default(),
            4,
            TextureWakes::default(),
            TestHooks::default(),
        )
        .expect("the texture ledger starts");
        let device = context.device();
        // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
        let (pool, event_one, event_three) = unsafe {
            (
                device
                    .create_command_pool(
                        &vk::CommandPoolCreateInfo::default().queue_family_index(context.family()),
                        None,
                    )
                    .expect("pool"),
                device
                    .create_event(&vk::EventCreateInfo::default(), None)
                    .expect("event"),
                device
                    .create_event(&vk::EventCreateInfo::default(), None)
                    .expect("event"),
            )
        };
        // A command buffer that waits on `event` (when given): the frame is held until the host sets it.
        let record = |event: Option<vk::Event>| {
            // SAFETY: the pool is live; the buffer is recorded once and submitted once.
            unsafe {
                let cmd = device
                    .allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .command_buffer_count(1),
                    )
                    .expect("cmd")[0];
                device
                    .begin_command_buffer(
                        cmd,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )
                    .expect("begin");
                if let Some(event) = event {
                    device.cmd_wait_events(
                        cmd,
                        &[event],
                        vk::PipelineStageFlags::HOST,
                        vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                        &[],
                        &[],
                        &[],
                    );
                }
                device.end_command_buffer(cmd).expect("end");
                cmd
            }
        };
        let proven = |number: u64| texture_ledger.frame_proven_number(number);
        let mut present = ledger();
        let chain = created(&mut present);
        let whole = present.snapshot();
        // Submits one frame through the texture ledger and numbers it in the present ledger.
        let submit = |present: &mut Ledger,
                      queue: &mut crate::RenderQueue,
                      hold: Option<vk::Event>|
         -> (Frame, u64) {
            let frame = present.take_frame().expect("the pools have room");
            present.acquired(0, chain);
            let ticket = texture_ledger.begin_frame().expect("a ticket");
            let number = ticket
                .submit(queue, record(hold), &[], &[])
                .expect("the frame is submitted")
                .number();
            present.submitted_numbered(&frame, number);
            (frame, number)
        };
        let pump_until = |queue: &mut crate::RenderQueue, number: u64| {
            let mut events = Vec::new();
            let started = Instant::now();
            while !texture_ledger.frame_proven_number(number) {
                texture_ledger.pump(queue, &mut events);
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "frame {number} never proven"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        };

        // Frame 1, held. The present ledger looks: not proven.
        let (first, one) = submit(&mut present, &mut queue, Some(event_one));
        assert_eq!(present.reap_numbered(&proven), 0, "frame 1 is unproven");
        let mut events = Vec::new();
        texture_ledger.pump(&mut queue, &mut events);
        assert_eq!(
            present.reap_numbered(&proven),
            0,
            "still unproven after a pump"
        );
        // The signal. The texture ledger reaps and latches frame 1 -- the present ledger does not look.
        // SAFETY: the event belongs to this device.
        unsafe { device.set_event(event_one) }.expect("set");
        pump_until(&mut queue, one);
        // Frame 2 on the other fence, proven; frame 3 on frame 1's fence, which `begin_frame` resets: held again.
        let (second, two) = submit(&mut present, &mut queue, None);
        pump_until(&mut queue, two);
        let (third, three) = submit(&mut present, &mut queue, Some(event_three));
        assert_eq!((one, two, three), (1, 2, 3));
        assert!(
            !texture_ledger.frame_proven_number(three),
            "frame 3 is on frame 1's reused fence and is not proven"
        );
        // Now the present ledger looks: it finds frame 1 (and 2) proven although the fence of frame 1 is frame 3's.
        assert_eq!(
            present.reap_numbered(&proven),
            2,
            "frames 1 and 2 are returned"
        );
        assert!(proven(one) && proven(two) && !proven(three));
        let mid = present.snapshot();
        assert_eq!(
            mid.free_acquire,
            whole.free_acquire - 1,
            "only frame 3's acquire semaphore is still out"
        );
        assert_eq!(present.proven_through(), 2);
        // Their present halves return the slots.
        for frame in [&first, &second] {
            present.presented(frame, PresentCall::Success);
            assert!(present.fence_signalled(frame.slot.fence).is_some());
        }
        // Frame 3's own signal.
        // SAFETY: the event belongs to this device.
        unsafe { device.set_event(event_three) }.expect("set");
        pump_until(&mut queue, three);
        assert_eq!(present.reap_numbered(&proven), 1);
        present.presented(&third, PresentCall::Success);
        assert!(present.fence_signalled(third.slot.fence).is_some());
        let end = present.snapshot();
        assert_eq!(
            (end.free_acquire, end.free_slots, end.free_fences),
            (whole.free_acquire, whole.free_slots, whole.free_fences),
            "every semaphore, slot and fence is back"
        );
        assert!(present.proven());
        assert_eq!(
            texture_ledger.shutdown(Instant::now() + Duration::from_secs(30)),
            crate::LedgerCleanup::Proven
        );
        // SAFETY: every frame that used them is proven finished.
        unsafe {
            device.destroy_event(event_one, None);
            device.destroy_event(event_three, None);
            device.destroy_command_pool(pool, None);
        }
        drop(texture_ledger);
        drop(queue);
        assert_eq!(Arc::strong_count(&context), 1);
        drop(context);
        assert_eq!(
            log.messages(),
            Vec::<String>::new(),
            "validation said nothing"
        );
        // SAFETY: there is no surface (null) and no presenter used the instance.
        unsafe { crate::destroy_surface_and_instance(instance, vk::SurfaceKHR::null(), None) }
            .expect("the instance is destroyed");
        drop(loader);
    }
}
