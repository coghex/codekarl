//! The map's bookkeeping (milestone 6, slice 1): pure, with no Vulkan call in it, so every rule is tested without a device.
//!
//! The render thread owns at most **one resident input** (grid A, grid B and the palette, with the staging buffers of its upload), **one
//! resident target**, and at most **one retired input and one retired target** (the presenter has one frame in flight, so no more than
//! one retired holder of each kind can still be in use):
//!
//! - an input is *resident* while it is the one the map pass reads; a target while it is the one the map pass writes and the window
//!   pass samples; each is *retired* once a new input or a new view size replaced it;
//! - a retired holder is destroyed once the ledger has the proof of the last submit that used it, and a holder that no submit ever used
//!   is destroyed at once;
//! - an input counts as *uploaded* only once the frame that recorded its upload has been **submitted**: a frame abandoned after
//!   recording leaves it not uploaded, its staging buffers kept, and the next frame records the upload again from them;
//! - the staging buffers are destroyed once the proof of the submit that carried the upload is in;
//! - a replacement that would retire a holder while the retired slot of its kind is taken waits: the frame is retried, nothing new is
//!   made, and nothing is destroyed early.
//!
//! Proofs are submit numbers (see the ledger): the caller passes the highest proven number, taken from the ledger's own record, never
//! from a fence handle.

/// What a frame's map needs that is not resident yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Need {
    /// A new input (its generation is not the resident one's).
    pub(crate) input: bool,
    /// A new target (its size is not the resident one's).
    pub(crate) target: bool,
}

/// Whether the replacements a frame needs may be made now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Room for them.
    Go,
    /// A retired holder of a kind this frame would retire another of is still waiting for its proof: the frame waits.
    Wait,
}

/// What became of the object a replacement displaced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Displaced {
    /// There was none, or it stays (not replaced).
    #[default]
    Nothing,
    /// It was never submitted, so nothing can use it: destroy it now.
    DestroyNow,
    /// It is retired, to be destroyed once its last submit is proven.
    Retired,
}

/// What replacing the resident objects did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Replaced {
    /// The old input.
    pub(crate) input: Displaced,
    /// The old target.
    pub(crate) target: Displaced,
}

/// What may be destroyed now, given the proofs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Reapable {
    /// The retired input.
    pub(crate) input: bool,
    /// The retired target.
    pub(crate) target: bool,
    /// The staging buffers of the resident input (its upload is proven finished).
    pub(crate) staging: bool,
}

#[derive(Debug)]
struct ResidentInput {
    generation: u64,
    uploaded: bool,
    /// The number of the last submit that read it.
    last_use: Option<u64>,
    /// The number of the submit that carried the upload, until the staging buffers are destroyed.
    upload: Option<u64>,
}

#[derive(Debug)]
struct ResidentTarget {
    size: [u32; 2],
    last_use: Option<u64>,
}

/// The bookkeeping (see the module documentation).
#[derive(Debug, Default)]
pub(crate) struct MapResidency {
    input: Option<ResidentInput>,
    target: Option<ResidentTarget>,
    /// The last use of the retired input / target (`Some`: it waits for that proof).
    retired_input: Option<u64>,
    retired_target: Option<u64>,
}

impl MapResidency {
    /// What `generation` and a `view` of this size need that is not resident.
    pub(crate) fn need(&self, generation: u64, view: [u32; 2]) -> Need {
        Need {
            input: self
                .input
                .as_ref()
                .is_none_or(|input| input.generation != generation),
            target: self
                .target
                .as_ref()
                .is_none_or(|target| target.size != view),
        }
    }

    /// Whether `need` may be met now: a replacement that retires a holder needs the retired slot of that kind free.
    pub(crate) fn admission(&self, need: Need) -> Admission {
        let blocked = (need.input && self.input.is_some() && self.retired_input.is_some())
            || (need.target && self.target.is_some() && self.retired_target.is_some());
        if blocked {
            Admission::Wait
        } else {
            Admission::Go
        }
    }

    /// The new objects exist: they become the resident ones, and the old ones are retired (or, if no submit ever used them, to be
    /// destroyed now). Only after [`admission`](Self::admission) said [`Admission::Go`].
    pub(crate) fn replace(&mut self, need: Need, generation: u64, view: [u32; 2]) -> Replaced {
        let mut replaced = Replaced::default();
        if need.input {
            let old = self.input.replace(ResidentInput {
                generation,
                uploaded: false,
                last_use: None,
                upload: None,
            });
            let (existed, last_use) = old.map_or((false, None), |old| (true, old.last_use));
            replaced.input = Self::displaced(existed, last_use, &mut self.retired_input);
        }
        if need.target {
            let old = self.target.replace(ResidentTarget {
                size: view,
                last_use: None,
            });
            let (existed, last_use) = old.map_or((false, None), |old| (true, old.last_use));
            replaced.target = Self::displaced(existed, last_use, &mut self.retired_target);
        }
        replaced
    }

    /// What becomes of an object that `existed` and whose last use was `last_use`: retired into `slot`, or destroyed now when no
    /// submit ever used it.
    fn displaced(existed: bool, last_use: Option<u64>, slot: &mut Option<u64>) -> Displaced {
        match (existed, last_use) {
            (false, _) => Displaced::Nothing,
            (true, None) => Displaced::DestroyNow,
            (true, Some(last)) => {
                *slot = Some(last);
                Displaced::Retired
            }
        }
    }

    /// Whether the resident input's upload is still to be recorded (it was never submitted).
    pub(crate) fn upload_pending(&self) -> bool {
        self.input.as_ref().is_some_and(|input| !input.uploaded)
    }

    /// The frame was submitted as `number`: the resident input and target are in use by it, and an input recorded in it is uploaded.
    pub(crate) fn submitted(&mut self, number: u64, upload_recorded: bool) {
        if let Some(input) = &mut self.input {
            input.last_use = Some(number);
            if upload_recorded {
                input.uploaded = true;
                input.upload = Some(number);
            }
        }
        if let Some(target) = &mut self.target {
            target.last_use = Some(number);
        }
    }

    /// What may be destroyed given the highest proven submit number.
    pub(crate) fn reapable(&self, proven_through: u64) -> Reapable {
        let proven = |last: Option<u64>| last.is_some_and(|last| last <= proven_through);
        Reapable {
            input: proven(self.retired_input),
            target: proven(self.retired_target),
            staging: proven(self.input.as_ref().and_then(|input| input.upload)),
        }
    }

    /// The retired input was destroyed.
    pub(crate) fn retired_input_destroyed(&mut self) {
        self.retired_input = None;
    }

    /// The retired target was destroyed.
    pub(crate) fn retired_target_destroyed(&mut self) {
        self.retired_target = None;
    }

    /// The resident input's staging buffers were destroyed.
    pub(crate) fn staging_destroyed(&mut self) {
        if let Some(input) = &mut self.input {
            input.upload = None;
        }
    }

    /// Whether a retired holder still waits for its proof.
    #[cfg(test)]
    pub(crate) fn retired_waiting(&self) -> bool {
        self.retired_input.is_some() || self.retired_target.is_some()
    }

    /// The resident input's generation, if there is one.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> Option<u64> {
        self.input.as_ref().map(|input| input.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::{Admission, Displaced, MapResidency, Need, Reapable, Replaced};

    const VIEW: [u32; 2] = [5, 3];
    const BIGGER: [u32; 2] = [6, 4];

    /// A residency with input generation 1 and a `VIEW` target, submitted as `number` with the upload.
    fn resident(number: u64) -> MapResidency {
        let mut residency = MapResidency::default();
        let need = residency.need(1, VIEW);
        assert_eq!(
            need,
            Need {
                input: true,
                target: true
            }
        );
        assert_eq!(residency.admission(need), Admission::Go);
        assert_eq!(residency.replace(need, 1, VIEW), Replaced::default());
        assert!(residency.upload_pending());
        residency.submitted(number, true);
        assert!(!residency.upload_pending());
        residency
    }

    #[test]
    fn the_same_generation_and_size_need_nothing() {
        let residency = resident(1);
        assert_eq!(
            residency.need(1, VIEW),
            Need {
                input: false,
                target: false
            }
        );
        assert!(residency.need(2, VIEW).input);
        assert!(residency.need(1, BIGGER).target);
    }

    #[test]
    fn an_upload_counts_only_once_the_frame_was_submitted() {
        let mut residency = MapResidency::default();
        let need = residency.need(1, VIEW);
        residency.replace(need, 1, VIEW);
        // A frame abandoned after recording the upload submits nothing: the upload is still pending for the next frame.
        assert!(residency.upload_pending());
        // A submit without the upload (the input already uploaded elsewhere cannot happen) leaves it pending.
        residency.submitted(1, false);
        assert!(residency.upload_pending());
        residency.submitted(2, true);
        assert!(!residency.upload_pending());
    }

    #[test]
    fn the_staging_goes_with_the_proof_of_the_upload_submit() {
        let mut residency = resident(4);
        assert_eq!(residency.reapable(3), Reapable::default());
        assert!(residency.reapable(4).staging);
        residency.staging_destroyed();
        assert!(!residency.reapable(9).staging, "destroyed once");
    }

    #[test]
    fn a_replaced_target_is_retired_and_destroyed_only_once_its_last_submit_is_proven() {
        let mut residency = resident(2);
        let need = residency.need(1, BIGGER);
        assert_eq!(
            need,
            Need {
                input: false,
                target: true
            }
        );
        assert_eq!(residency.admission(need), Admission::Go);
        let replaced = residency.replace(need, 1, BIGGER);
        assert_eq!(
            replaced,
            Replaced {
                target: Displaced::Retired,
                ..Replaced::default()
            }
        );
        assert!(residency.retired_waiting());
        assert!(
            !residency.reapable(1).target,
            "its last use, submit 2, is not proven"
        );
        assert!(residency.reapable(2).target);
        residency.retired_target_destroyed();
        assert!(!residency.retired_waiting());
    }

    #[test]
    fn a_second_replacement_waits_while_the_retired_holder_of_its_kind_waits() {
        let mut residency = resident(2);
        let need = residency.need(1, BIGGER);
        residency.replace(need, 1, BIGGER);
        residency.submitted(3, false);
        // Another resize while the first retired target is not proven: the frame waits, and nothing changes.
        let again = residency.need(1, [7, 5]);
        assert_eq!(residency.admission(again), Admission::Wait);
        // A new input is another kind: it may go ahead.
        let input = Need {
            input: true,
            target: false,
        };
        assert_eq!(residency.admission(input), Admission::Go);
        // Once the retired target is proven and destroyed, the resize goes.
        assert!(residency.reapable(2).target);
        residency.retired_target_destroyed();
        assert_eq!(residency.admission(again), Admission::Go);
    }

    #[test]
    fn an_object_no_submit_used_is_destroyed_at_once_not_retired() {
        let mut residency = MapResidency::default();
        let need = residency.need(1, VIEW);
        residency.replace(need, 1, VIEW);
        // The first frame never submitted, and the view changes: the old input and target are destroyed now.
        let need = residency.need(2, BIGGER);
        let replaced = residency.replace(need, 2, BIGGER);
        assert_eq!(
            replaced,
            Replaced {
                input: Displaced::DestroyNow,
                target: Displaced::DestroyNow,
            }
        );
        assert!(!residency.retired_waiting());
        assert_eq!(residency.generation(), Some(2));
    }

    #[test]
    fn a_new_input_retires_the_old_one_with_the_old_submit() {
        let mut residency = resident(5);
        let need = residency.need(2, VIEW);
        let replaced = residency.replace(need, 2, VIEW);
        assert_eq!(
            (replaced.input, replaced.target),
            (Displaced::Retired, Displaced::Nothing)
        );
        assert!(
            residency.upload_pending(),
            "the new input is not uploaded yet"
        );
        assert!(!residency.reapable(4).input);
        assert!(residency.reapable(5).input);
    }
}
