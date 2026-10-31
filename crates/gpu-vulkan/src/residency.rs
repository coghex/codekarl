//! The quad textures' bookkeeping (design §7.2): pure, with no Vulkan call in it, so every rule is tested
//! without a device.
//!
//! The render thread owns at most **two textures** (one resident, one retired) and **one staging buffer**:
//! - a texture is *resident* while it is the one being drawn, and *retired* once it was replaced or removed;
//! - a retired texture is destroyed once the ledger has the proof of the last submit that drew with it;
//! - a staging buffer is destroyed once the proof of the submit that carried its upload is in;
//! - a texture counts as *uploaded* only once the frame that recorded its upload has been **submitted**: a frame
//!   abandoned after recording leaves it not uploaded and its staging buffer kept, and the next frame records the
//!   upload again from the kept bytes;
//! - no third texture or second staging buffer is created while the cap is reached: the frame waits.
//!
//! Proofs are submit numbers (see the ledger): the caller passes the highest proven number, taken from the
//! ledger's own record, never from a fence handle.

use std::sync::{Arc, Weak};

use crate::quad::{PixelSource, VulkanQuad};

/// The most textures alive at once.
pub(crate) const MAX_TEXTURES: usize = 2;
/// The most staging buffers alive at once.
pub(crate) const MAX_STAGING: usize = 1;

/// An image's identity: the pixel source's outer `Arc` allocation plus the image's width and height. Held as a
/// `Weak`: that keeps the allocation (so its address is never reused for another source while this lives), but not
/// the pixels.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    image: Weak<dyn PixelSource>,
    width: u32,
    height: u32,
}

impl Identity {
    pub(crate) fn of(quad: &VulkanQuad) -> Self {
        Self {
            image: Arc::downgrade(quad.pixels()),
            width: quad.width(),
            height: quad.height(),
        }
    }

    /// Whether `quad`'s image is this one.
    pub(crate) fn matches(&self, quad: &VulkanQuad) -> bool {
        std::ptr::addr_eq(self.image.as_ptr(), Arc::as_ptr(quad.pixels()))
            && (self.width, self.height) == (quad.width(), quad.height())
    }
}

/// Which texture owns the staging buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    Resident,
    Retired,
}

#[derive(Debug)]
struct Resident {
    identity: Identity,
    uploaded: bool,
    /// The number of the last submit that drew with it.
    last_use: Option<u64>,
}

#[derive(Debug)]
struct Retired {
    last_use: Option<u64>,
}

#[derive(Debug)]
struct Staging {
    owner: Owner,
    /// The number of the submit that carried the upload, once it was made.
    upload: Option<u64>,
}

/// Whether a new texture may be created now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Room for the texture and its staging buffer.
    Create,
    /// The cap is reached: the frame waits, its redraw kept.
    Wait,
}

/// What retiring the resident texture (the scene shows no quad) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Retirement {
    /// There was no resident texture.
    Nothing,
    /// It was never submitted, so nothing can use it: destroy it (and its staging buffer) now.
    DestroyNow,
    /// It is retired, to be destroyed once its last submit is proven.
    Retired,
    /// The retired slot is taken: the resident stays until that one is destroyed.
    Blocked,
}

/// What may be destroyed now, given the proofs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Reapable {
    /// The retired texture (and its staging buffer, if it still owns it).
    pub(crate) retired: bool,
    /// The staging buffer, whoever owns it.
    pub(crate) staging: Option<Owner>,
}

/// What replacing the resident texture did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replaced {
    /// There was no resident texture.
    Nothing,
    /// The old one was never submitted: destroy it (and its staging buffer) now.
    DestroyOld,
    /// The old one is retired.
    RetiredOld,
}

/// The bookkeeping (see the module documentation).
#[derive(Debug, Default)]
pub(crate) struct Residency {
    resident: Option<Resident>,
    retired: Option<Retired>,
    staging: Option<Staging>,
}

impl Residency {
    /// Textures alive (resident and retired).
    #[cfg(test)]
    pub(crate) fn textures(&self) -> usize {
        usize::from(self.resident.is_some()) + usize::from(self.retired.is_some())
    }

    /// Staging buffers alive.
    #[cfg(test)]
    pub(crate) fn staging_buffers(&self) -> usize {
        usize::from(self.staging.is_some())
    }

    #[cfg(test)]
    pub(crate) fn has_retired(&self) -> bool {
        self.retired.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_resident(&self) -> bool {
        self.resident.is_some()
    }

    /// Whether `quad`'s image is the resident texture's.
    pub(crate) fn resident_is(&self, quad: &VulkanQuad) -> bool {
        self.resident
            .as_ref()
            .is_some_and(|resident| resident.identity.matches(quad))
    }

    /// Whether the resident texture's upload has not been submitted yet, so the next frame must record it (from
    /// its kept staging buffer).
    pub(crate) fn upload_pending(&self) -> bool {
        self.resident
            .as_ref()
            .is_some_and(|resident| !resident.uploaded)
    }

    /// Whether the resident texture may be refiltered: no submit that drew with it is still unproven.
    pub(crate) fn refilter_allowed(&self, proven: u64) -> bool {
        self.resident
            .as_ref()
            .is_none_or(|resident| resident.last_use.is_none_or(|last| last <= proven))
    }

    /// Whether a new texture and its staging buffer may be created now. A resident texture that was never
    /// submitted is destroyed by the replacement, and does not count.
    pub(crate) fn admission(&self) -> Admission {
        let unused = self
            .resident
            .as_ref()
            .is_some_and(|resident| resident.last_use.is_none());
        let textures =
            usize::from(self.resident.is_some() && !unused) + usize::from(self.retired.is_some());
        let staging = self
            .staging
            .as_ref()
            .is_some_and(|staging| !(unused && staging.owner == Owner::Resident));
        if textures < MAX_TEXTURES && usize::from(staging) < MAX_STAGING {
            Admission::Create
        } else {
            Admission::Wait
        }
    }

    /// A new texture for `identity` replaces the resident one (the caller checked [`admission`](Self::admission)).
    /// Its staging buffer is created with it.
    pub(crate) fn replace(&mut self, identity: Identity) -> Replaced {
        let replaced = match self.resident.take() {
            None => Replaced::Nothing,
            Some(old) if old.last_use.is_none() => {
                if self
                    .staging
                    .as_ref()
                    .is_some_and(|staging| staging.owner == Owner::Resident)
                {
                    self.staging = None;
                }
                Replaced::DestroyOld
            }
            Some(old) => {
                self.retired = Some(Retired {
                    last_use: old.last_use,
                });
                if let Some(staging) = &mut self.staging
                    && staging.owner == Owner::Resident
                {
                    staging.owner = Owner::Retired;
                }
                Replaced::RetiredOld
            }
        };
        self.resident = Some(Resident {
            identity,
            uploaded: false,
            last_use: None,
        });
        self.staging = Some(Staging {
            owner: Owner::Resident,
            upload: None,
        });
        replaced
    }

    /// The scene shows no quad: retire the resident texture.
    pub(crate) fn retire_resident(&mut self) -> Retirement {
        let Some(old) = &self.resident else {
            return Retirement::Nothing;
        };
        if old.last_use.is_none() {
            self.resident = None;
            if self
                .staging
                .as_ref()
                .is_some_and(|staging| staging.owner == Owner::Resident)
            {
                self.staging = None;
            }
            return Retirement::DestroyNow;
        }
        if self.retired.is_some() {
            return Retirement::Blocked;
        }
        let last_use = old.last_use;
        self.resident = None;
        self.retired = Some(Retired { last_use });
        if let Some(staging) = &mut self.staging
            && staging.owner == Owner::Resident
        {
            staging.owner = Owner::Retired;
        }
        Retirement::Retired
    }

    /// The frame that drew the resident texture was submitted as `number`; `with_upload` says it carried the
    /// upload.
    pub(crate) fn submitted(&mut self, number: u64, with_upload: bool) {
        let Some(resident) = &mut self.resident else {
            return;
        };
        resident.last_use = Some(number);
        if with_upload {
            resident.uploaded = true;
            if let Some(staging) = &mut self.staging
                && staging.owner == Owner::Resident
            {
                staging.upload = Some(number);
            }
        }
    }

    /// What is destroyable now, given the highest proven submit number.
    pub(crate) fn reapable(&self, proven: u64) -> Reapable {
        Reapable {
            retired: self
                .retired
                .as_ref()
                .is_some_and(|retired| retired.last_use.is_none_or(|last| last <= proven)),
            staging: self.staging.as_ref().and_then(|staging| {
                staging
                    .upload
                    .is_some_and(|upload| upload <= proven)
                    .then_some(staging.owner)
            }),
        }
    }

    /// The retired texture (with the staging buffer it owned) was destroyed.
    pub(crate) fn retired_destroyed(&mut self) {
        self.retired = None;
        if self
            .staging
            .as_ref()
            .is_some_and(|staging| staging.owner == Owner::Retired)
        {
            self.staging = None;
        }
    }

    /// The staging buffer was destroyed on its own.
    pub(crate) fn staging_destroyed(&mut self) {
        self.staging = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::quad::{DestinationRect, SourceRect, VulkanFilter};

    struct Bytes(Vec<u8>);

    impl PixelSource for Bytes {
        fn pixels(&self) -> &[u8] {
            &self.0
        }
    }

    fn quad_of(source: &Arc<Bytes>, width: u32, height: u32) -> VulkanQuad {
        VulkanQuad::new(
            Arc::clone(source) as Arc<dyn PixelSource>,
            width,
            height,
            DestinationRect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            SourceRect {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            VulkanFilter::Nearest,
        )
        .expect("a valid quad")
    }

    fn image(width: u32, height: u32) -> (Arc<Bytes>, VulkanQuad) {
        let source = Arc::new(Bytes(vec![0; (width * height * 4) as usize]));
        let quad = quad_of(&source, width, height);
        (source, quad)
    }

    /// A residency with an uploaded, drawn resident texture: submit 1 carried its upload.
    fn drawn() -> (Residency, Arc<Bytes>, VulkanQuad) {
        let (source, quad) = image(1, 1);
        let mut residency = Residency::default();
        assert_eq!(residency.admission(), Admission::Create);
        assert_eq!(residency.replace(Identity::of(&quad)), Replaced::Nothing);
        residency.submitted(1, true);
        (residency, source, quad)
    }

    #[test]
    fn identity_is_the_outer_allocation_plus_the_size() {
        let (source, quad) = image(2, 1);
        let identity = Identity::of(&quad);
        assert!(identity.matches(&quad));
        assert!(
            identity.matches(&quad_of(&source, 2, 1)),
            "the same allocation"
        );
        let (_other, other_quad) = image(2, 1);
        assert!(
            !identity.matches(&other_quad),
            "another allocation of the same size is another image"
        );
        // The same allocation, read as another size, is another image.
        let wide = Arc::new(Bytes(vec![0; 16]));
        let as_two_by_two = quad_of(&wide, 2, 2);
        let as_four_by_one = quad_of(&wide, 4, 1);
        assert!(!Identity::of(&as_two_by_two).matches(&as_four_by_one));
    }

    #[test]
    fn identity_keeps_the_allocation_but_not_the_pixels() {
        let (source, quad) = image(1, 1);
        let identity = Identity::of(&quad);
        drop(quad);
        assert_eq!(Arc::strong_count(&source), 1, "no strong reference");
        let address = Arc::as_ptr(&source).cast::<()>();
        drop(source);
        // The weak reference keeps the allocation alive, so its address cannot be handed to a new source.
        let (_new, new_quad) = image(1, 1);
        assert_ne!(Arc::as_ptr(new_quad.pixels()).cast::<()>(), address);
        assert!(!identity.matches(&new_quad));
    }

    #[test]
    fn the_first_texture_is_admitted_and_is_pending_its_upload() {
        let (_source, quad) = image(1, 1);
        let mut residency = Residency::default();
        assert_eq!(residency.admission(), Admission::Create);
        residency.replace(Identity::of(&quad));
        assert_eq!((residency.textures(), residency.staging_buffers()), (1, 1));
        assert!(residency.upload_pending());
        assert!(residency.resident_is(&quad));
        assert_eq!(
            residency.reapable(u64::MAX),
            Reapable::default(),
            "staging waits for its upload to be submitted, however much is proven"
        );
    }

    #[test]
    fn a_texture_is_uploaded_only_once_its_frame_is_submitted() {
        let (mut residency, _source, _quad) = drawn();
        assert!(!residency.upload_pending());
        // A fresh residency whose frame was abandoned before the submit: nothing was marked.
        let (_source, quad) = image(1, 1);
        let mut abandoned = Residency::default();
        abandoned.replace(Identity::of(&quad));
        assert!(abandoned.upload_pending(), "still to be uploaded");
        assert_eq!(abandoned.reapable(u64::MAX), Reapable::default());
        assert!(abandoned.refilter_allowed(0), "nothing submitted used it");
        abandoned.submitted(1, true);
        assert!(!abandoned.upload_pending());
        residency.submitted(2, false);
        assert!(!residency.upload_pending());
    }

    #[test]
    fn the_staging_buffer_goes_once_the_upload_submit_is_proven() {
        let (mut residency, _source, _quad) = drawn();
        assert_eq!(
            residency.reapable(0).staging,
            None,
            "submit 1 is not proven"
        );
        assert_eq!(residency.reapable(1).staging, Some(Owner::Resident));
        residency.staging_destroyed();
        assert_eq!(residency.staging_buffers(), 0);
        assert_eq!(residency.textures(), 1, "the texture stays");
    }

    #[test]
    fn a_replaced_texture_is_retired_with_its_last_submit_and_destroyed_only_after_that_proof() {
        let (mut residency, _source, _quad) = drawn();
        residency.submitted(2, false);
        // The first upload's staging buffer must be proven and gone before another one is admitted.
        assert_eq!(residency.admission(), Admission::Wait);
        assert_eq!(residency.reapable(1).staging, Some(Owner::Resident));
        residency.staging_destroyed();
        let (_next_source, next) = image(2, 2);
        assert_eq!(
            residency.admission(),
            Admission::Create,
            "one retired is allowed"
        );
        assert_eq!(residency.replace(Identity::of(&next)), Replaced::RetiredOld);
        assert_eq!(residency.textures(), MAX_TEXTURES);
        assert!(residency.has_retired());
        // The old texture's last use is submit 2.
        assert!(!residency.reapable(1).retired);
        assert!(residency.reapable(2).retired);
        assert_eq!(
            residency.staging_buffers(),
            1,
            "the new texture's own staging buffer"
        );
    }

    #[test]
    fn no_third_texture_is_admitted_while_the_cap_is_reached() {
        let (mut residency, _source, _quad) = drawn();
        residency.submitted(2, false);
        residency.staging_destroyed();
        let (_s1, second) = image(1, 2);
        residency.replace(Identity::of(&second));
        residency.submitted(3, true);
        residency.staging_destroyed();
        assert_eq!(residency.textures(), 2);
        assert_eq!(
            residency.admission(),
            Admission::Wait,
            "a resident that was drawn and a retired one fill the cap"
        );
        // The retired texture's last use (2) is proven: it goes, and the cap has room again.
        assert!(residency.reapable(2).retired);
        residency.retired_destroyed();
        assert_eq!(residency.textures(), 1);
        assert_eq!(residency.admission(), Admission::Create);
    }

    #[test]
    fn a_second_staging_buffer_waits_for_the_first_to_be_proven() {
        let (mut residency, _source, _quad) = drawn();
        // Submit 1 carried the first upload and is not proven: its staging buffer is still alive, so no
        // second one is admitted, whatever room the textures have.
        assert_eq!(residency.staging_buffers(), 1);
        assert_eq!(residency.admission(), Admission::Wait);
        assert_eq!(residency.reapable(0).staging, None);
        assert_eq!(residency.reapable(1).staging, Some(Owner::Resident));
        residency.staging_destroyed();
        assert_eq!(residency.admission(), Admission::Create);
    }

    #[test]
    fn a_resident_that_was_never_submitted_is_destroyed_by_its_replacement_and_does_not_count() {
        let (_source, first) = image(1, 1);
        let mut residency = Residency::default();
        residency.replace(Identity::of(&first));
        // Abandoned frames: nothing was submitted. Another image arrives.
        assert_eq!(residency.admission(), Admission::Create);
        let (_other_source, other) = image(3, 1);
        assert_eq!(
            residency.replace(Identity::of(&other)),
            Replaced::DestroyOld
        );
        assert_eq!((residency.textures(), residency.staging_buffers()), (1, 1));
        assert!(residency.resident_is(&other));
        assert!(!residency.resident_is(&first));
    }

    #[test]
    fn a_scene_without_a_quad_retires_a_used_texture_and_destroys_an_unused_one_at_once() {
        let (mut residency, _source, _quad) = drawn();
        assert_eq!(residency.retire_resident(), Retirement::Retired);
        assert_eq!(
            (residency.has_resident(), residency.has_retired()),
            (false, true)
        );
        assert_eq!(residency.reapable(1).staging, Some(Owner::Retired));
        assert!(residency.reapable(1).retired);
        assert_eq!(residency.retire_resident(), Retirement::Nothing);

        let (_s, quad) = image(1, 1);
        let mut unused = Residency::default();
        unused.replace(Identity::of(&quad));
        assert_eq!(unused.retire_resident(), Retirement::DestroyNow);
        assert_eq!((unused.textures(), unused.staging_buffers()), (0, 0));
    }

    #[test]
    fn a_resident_cannot_retire_while_the_retired_slot_is_taken() {
        let (mut residency, _source, _quad) = drawn();
        let (_s, second) = image(2, 2);
        residency.replace(Identity::of(&second));
        residency.submitted(2, true);
        assert!(residency.has_retired());
        assert_eq!(residency.retire_resident(), Retirement::Blocked);
        assert!(residency.has_resident(), "it stays resident, undrawn");
        residency.retired_destroyed();
        assert_eq!(residency.retire_resident(), Retirement::Retired);
    }

    #[test]
    fn a_refilter_waits_for_the_last_submit_that_drew_the_texture() {
        let (mut residency, _source, _quad) = drawn();
        assert!(!residency.refilter_allowed(0));
        assert!(residency.refilter_allowed(1));
        residency.submitted(2, false);
        assert!(!residency.refilter_allowed(1));
        assert!(residency.refilter_allowed(2));
    }

    #[test]
    fn the_retired_textures_staging_goes_with_it() {
        let (mut residency, _source, _quad) = drawn();
        assert_eq!(residency.retire_resident(), Retirement::Retired);
        residency.retired_destroyed();
        assert_eq!((residency.textures(), residency.staging_buffers()), (0, 0));
    }
}
