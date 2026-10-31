//! Frame completion as a recorded submission number, never a raw fence (milestone 5.5-1, review 1 finding F2).
//!
//! A frame fence is reused: it is reset when a later ticket takes it, so its status says nothing about an earlier
//! submission once it was reset. What outlives the reset is the *number* of the submission whose completion was
//! seen. This is pure bookkeeping over fence handles of any type, so every rule is tested without a device:
//!
//! - the ledger owns a pool of fences (two); a submission gets the next number from a monotonic counter;
//! - only the ledger's `pump` and `prove` read fence status, and when they see one signalled they **record the
//!   proof** here ([`FenceProofs::record_proven`]) before anything else is done with that fence;
//! - at most one submitted ticket is pending, so proofs arrive in submission order and the highest proven number
//!   is the whole record: [`FenceProofs::is_proven`] is true once a submission's proof is recorded, stays true
//!   after its fence is reset and reused, and is never true for a later submission because an earlier fence
//!   signalled;
//! - a fence is taken for a new ticket only if its previous submission's proof is already recorded
//!   ([`FenceProofs::take`]), and the caller resets it then and only then.

/// What [`FenceProofs::take`] hands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Taken {
    /// The slot taken.
    pub(crate) slot: usize,
    /// The fence was used before: its previous submission is proven, and the caller resets it now.
    pub(crate) reset: bool,
}

#[derive(Debug, Clone, Copy)]
struct Slot<F> {
    fence: F,
    /// The number of the last submission that used this fence (`None` before the first).
    last: Option<u64>,
    /// A building ticket holds the slot.
    held: bool,
}

/// The fence pool and the proofs recorded against it.
#[derive(Debug)]
pub(crate) struct FenceProofs<F: Copy> {
    slots: Vec<Slot<F>>,
    next: u64,
    proven: u64,
}

impl<F: Copy> FenceProofs<F> {
    /// A pool over `fences`, none used yet.
    pub(crate) fn new(fences: &[F]) -> Self {
        Self {
            slots: fences
                .iter()
                .map(|&fence| Slot {
                    fence,
                    last: None,
                    held: false,
                })
                .collect(),
            next: 1,
            proven: 0,
        }
    }

    /// Takes a fence for a new ticket: one nobody holds whose previous submission's proof is recorded (or that
    /// was never used). `None` when there is none, which the ledger reports as busy.
    pub(crate) fn take(&mut self) -> Option<Taken> {
        let proven = self.proven;
        let slot = self
            .slots
            .iter()
            .position(|slot| !slot.held && slot.last.is_none_or(|last| last <= proven))?;
        let reset = self.slots[slot].last.is_some();
        self.slots[slot].held = true;
        Some(Taken { slot, reset })
    }

    /// The taken fence was reset: it has no previous submission any more.
    pub(crate) fn was_reset(&mut self, slot: usize) {
        self.slots[slot].last = None;
    }

    /// A ticket that held `slot` ended without submitting: the fence was never used.
    pub(crate) fn untake(&mut self, slot: usize) {
        self.slots[slot].held = false;
    }

    /// The ticket that held `slot` submitted: the next number, now the slot's last submission.
    pub(crate) fn submitted(&mut self, slot: usize) -> u64 {
        let number = self.next;
        self.next += 1;
        self.slots[slot].last = Some(number);
        self.slots[slot].held = false;
        number
    }

    /// The fence in `slot`.
    pub(crate) fn fence(&self, slot: usize) -> F {
        self.slots[slot].fence
    }

    /// Every fence of the pool.
    pub(crate) fn fences(&self) -> Vec<F> {
        self.slots.iter().map(|slot| slot.fence).collect()
    }

    /// Records the proof of submission `number` (a fence was seen signalled): this and every earlier one.
    pub(crate) fn record_proven(&mut self, number: u64) {
        self.proven = self.proven.max(number);
    }

    /// Whether the proof of submission `number` is recorded. It stays true after the fence is reset and reused.
    pub(crate) fn is_proven(&self, number: u64) -> bool {
        number != 0 && number <= self.proven
    }
}

#[cfg(test)]
mod tests {
    use super::{FenceProofs, Taken};

    #[test]
    fn a_fence_is_taken_only_after_its_previous_proof_is_recorded() {
        let mut pool = FenceProofs::new(&[10_u32, 11]);
        let first = pool.take().expect("a fresh fence");
        assert_eq!(
            first,
            Taken {
                slot: 0,
                reset: false
            }
        );
        let one = pool.submitted(first.slot);
        let second = pool.take().expect("the other fence");
        assert_eq!(
            second,
            Taken {
                slot: 1,
                reset: false
            }
        );
        let two = pool.submitted(second.slot);
        assert_eq!((one, two), (1, 2));
        // Neither submission has a recorded proof: no fence may be reset, so the pool is busy.
        assert_eq!(pool.take(), None, "both previous proofs are unrecorded");
        pool.record_proven(one);
        assert_eq!(
            pool.take(),
            Some(Taken {
                slot: 0,
                reset: true
            }),
            "submission 1 is proven, so its fence is reset for the next ticket"
        );
        // The second fence still waits for its proof.
        assert_eq!(pool.take(), None);
    }

    #[test]
    fn a_proof_stays_true_after_the_fence_is_reset_and_reused() {
        let mut pool = FenceProofs::new(&[10_u32, 11]);
        let mut numbers = Vec::new();
        for _ in 0..3 {
            let taken = pool.take().expect("a free fence");
            if taken.reset {
                pool.was_reset(taken.slot);
            }
            let number = pool.submitted(taken.slot);
            numbers.push(number);
            if numbers.len() < 3 {
                pool.record_proven(number);
            }
        }
        // Submission 3 reused submission 1's fence (slot 0) and is held; 1 and 2 are proven.
        assert!(
            pool.is_proven(numbers[0]),
            "n stays proven after its fence was reset and reused"
        );
        assert!(pool.is_proven(numbers[1]));
        assert!(!pool.is_proven(numbers[2]), "n + 2 is held");
        pool.record_proven(numbers[2]);
        assert!(pool.is_proven(numbers[2]));
        assert!(!pool.is_proven(0), "no submission has number zero");
        assert!(
            !pool.is_proven(4),
            "a submission that was never made is not proven"
        );
    }

    #[test]
    fn an_abandoned_ticket_frees_its_fence_and_takes_no_number() {
        let mut pool = FenceProofs::new(&[10_u32, 11]);
        let taken = pool.take().expect("a fence");
        pool.untake(taken.slot);
        let again = pool.take().expect("the same fence, unused");
        assert_eq!(
            again,
            Taken {
                slot: 0,
                reset: false
            }
        );
        assert_eq!(
            pool.submitted(again.slot),
            1,
            "no number was spent on the abandoned ticket"
        );
    }

    /// Negative control: answering "is `n` proven" from the status of the fence `n` used reports `n` unproven
    /// once that fence was reset for a later submission, and reports a held submission proven when its reused
    /// fence still shows the old signal. The recorded number answers both correctly.
    #[test]
    fn negative_control_answering_from_the_current_fence_status_is_wrong_after_reuse() {
        /// The naive answer: the status of the fence the submission used.
        fn naive(fence_of: &[(u64, usize)], signalled: [bool; 2], number: u64) -> bool {
            let (_, fence) = fence_of
                .iter()
                .find(|(held, _)| *held == number)
                .expect("a known submission");
            signalled[*fence]
        }
        let mut pool = FenceProofs::new(&[0_usize, 1]);
        let mut signalled = [false; 2];
        let mut fence_of: Vec<(u64, usize)> = Vec::new();
        // Submission 1 on fence 0 and submission 2 on fence 1 complete and are recorded.
        for flag in &mut signalled {
            let taken = pool.take().expect("a free fence");
            let number = pool.submitted(taken.slot);
            fence_of.push((number, pool.fence(taken.slot)));
            *flag = true;
            pool.record_proven(number);
        }
        // Submission 3 takes fence 0 again: reset, then held (unsignalled).
        let taken = pool.take().expect("fence 0 again");
        assert!(taken.reset);
        pool.was_reset(taken.slot);
        signalled[0] = false;
        let three = pool.submitted(taken.slot);
        fence_of.push((three, pool.fence(taken.slot)));
        assert!(
            !naive(&fence_of, signalled, 1),
            "the naive answer says submission 1 is unproven after its fence was reset"
        );
        assert!(
            pool.is_proven(1),
            "the recorded number still says it is proven"
        );
        assert!(
            !naive(&fence_of, signalled, three) && !pool.is_proven(three),
            "the held one is unproven"
        );
        // Submission 3 completes and is recorded; submission 4 takes the first free fence, again fence 0. Suppose
        // the naive reader still sees the old signal there (a reset it did not wait for).
        pool.record_proven(three);
        let taken = pool.take().expect("a fence again");
        assert!(taken.reset);
        pool.was_reset(taken.slot);
        signalled[taken.slot] = true;
        let four = pool.submitted(taken.slot);
        fence_of.push((four, pool.fence(taken.slot)));
        assert!(
            naive(&fence_of, signalled, four),
            "the naive answer reports the held submission 4 proven from the stale signal"
        );
        assert!(!pool.is_proven(four), "the recorded number does not");
    }
}
