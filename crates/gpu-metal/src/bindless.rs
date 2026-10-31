//! Bindless capability limits and the queue-attached residency set.

use std::{
    collections::VecDeque,
    ffi::{c_char, c_void},
    ptr::NonNull,
};

use crate::{Error, textures::MetalContext};

pub(crate) const RESIDENCY_CHANGES_PER_PASS: usize = 1_024;
pub(crate) const TABLE_WRITES_PER_PASS: usize = 4_096;

unsafe extern "C" {
    fn ck_mc_tables_new(
        context: *mut c_void,
        slots: u32,
        allocated: *mut u64,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_tables_buffer(tables: *mut c_void, version: u32) -> *mut c_void;
    fn ck_mc_tables_write(tables: *mut c_void, version: u32, slot: u32, resource_id: u64) -> i32;
    fn ck_mc_tables_free(tables: *mut c_void);
}

unsafe extern "C" {
    fn ck_mc_residency_new(
        context: *mut c_void,
        slots: u32,
        force_tier1: i32,
        fail_creation: i32,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_residency_add(set: *mut c_void, texture: *mut c_void);
    fn ck_mc_residency_remove(set: *mut c_void, texture: *mut c_void);
    fn ck_mc_residency_commit(set: *mut c_void);
    fn ck_mc_residency_commit_count(set: *mut c_void) -> u64;
    fn ck_mc_residency_size(set: *mut c_void) -> u64;
    fn ck_mc_residency_free(set: *mut c_void);
}

/// Numeric limits supplied by the caller for one bindless run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindlessLimits {
    /// Number of native resource-ID slots, including checker slot zero.
    pub texture_slots: u32,
    /// Maximum logical loaded textures.
    pub max_textures: u32,
    /// Maximum additional native revisions.
    pub max_extra_revisions: u32,
}

impl BindlessLimits {
    /// Validates the slot range and required capacity without overflowing.
    ///
    /// # Errors
    ///
    /// Returns an error when the slot range is unsupported or the requested logical capacity does not fit.
    pub fn validate(self) -> Result<Self, Error> {
        if !(2..=1_000_000).contains(&self.texture_slots) {
            return Err(Error(format!(
                "bindless texture_slots={} is outside 2..=1000000",
                self.texture_slots
            )));
        }
        let required = self
            .max_textures
            .checked_add(self.max_extra_revisions)
            .and_then(|sum| sum.checked_add(1));
        match required {
            Some(required) if self.texture_slots >= required => Ok(self),
            Some(required) => Err(Error(format!(
                "bindless texture_slots={} is below max_textures={} + max_extra_revisions={} + checker=1 (required {required})",
                self.texture_slots, self.max_textures, self.max_extra_revisions
            ))),
            None => Err(Error(format!(
                "bindless capacity overflow: texture_slots={}, max_textures={}, max_extra_revisions={}",
                self.texture_slots, self.max_textures, self.max_extra_revisions
            ))),
        }
    }
}

/// Queue-attached native residency set. Membership changes are owner-thread operations.
pub(crate) struct ResidencySet {
    handle: NonNull<c_void>,
}

/// Two native shared-storage resource-ID tables and their bounded CPU catch-up state.
pub(crate) struct ResourceTables {
    handle: NonNull<c_void>,
    slots: u32,
    buffers: [*mut c_void; 2],
    allocated: u64,
    current: usize,
    epoch: u64,
    version_epochs: [u64; 2],
    wanted: Vec<u64>,
    staged: Vec<bool>,
    contents: [Vec<u64>; 2],
    dirty: [VecDeque<u32>; 2],
    queued: [Vec<bool>; 2],
    changed: Vec<u32>,
    changed_marked: Vec<bool>,
}

/// Frame-scoped pointer and epoch for the table version pinned by a ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableView {
    /// Borrowed native shared-storage Metal buffer, valid while the ticket or its submitted completion is live.
    pub buffer: *mut c_void,
    /// Monotonic table publication epoch.
    pub epoch: u64,
    /// Number of native resource-ID slots, including checker slot zero.
    pub slots: u32,
}

/// Native slot zero is reserved for the checker; all texture revisions use this bounded pool.
pub(crate) struct ResourceSlots {
    free: Vec<u32>,
    capacity: usize,
}

/// Bounds the native membership work performed by one owner pass.
pub(crate) struct MembershipBudget(usize);

impl MembershipBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self(limit)
    }

    pub(crate) fn remaining(&self) -> usize {
        self.0
    }

    pub(crate) fn take(&mut self) -> bool {
        if self.0 == 0 {
            false
        } else {
            self.0 -= 1;
            true
        }
    }
}

impl ResourceSlots {
    pub(crate) fn new(texture_slots: u32) -> Self {
        Self {
            free: (1..texture_slots).rev().collect(),
            capacity: usize::try_from(texture_slots.saturating_sub(1)).unwrap_or(usize::MAX),
        }
    }

    pub(crate) fn allocate(&mut self) -> Option<u32> {
        self.free.pop()
    }

    pub(crate) fn release(&mut self, slot: u32) {
        debug_assert_ne!(slot, 0, "checker slot cannot be released");
        self.free.push(slot);
    }

    pub(crate) fn in_use(&self) -> usize {
        self.capacity().saturating_sub(self.free.len())
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }
}

impl ResidencySet {
    pub(crate) fn start(
        context: &MetalContext,
        limits: BindlessLimits,
        force_tier1: bool,
        fail_creation: bool,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        // Reserve the texture slots plus the two immutable tables and the instance buffer added by M5-10.
        let residency_capacity = limits
            .texture_slots
            .checked_add(3)
            .ok_or_else(|| Error("bindless residency capacity overflow".to_owned()))?;
        let mut error = [0_u8; 1024];
        // SAFETY: the context stays retained by this call and the native function bounds its error write.
        let handle = unsafe {
            ck_mc_residency_new(
                context.raw(),
                residency_capacity,
                i32::from(force_tier1),
                i32::from(fail_creation),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let handle = NonNull::new(handle).ok_or_else(|| crate::message(&error))?;
        Ok(Self { handle })
    }

    pub(crate) fn add(&mut self, texture: *mut c_void) {
        // SAFETY: owner-thread caller keeps the texture alive until a matching committed removal.
        unsafe { ck_mc_residency_add(self.handle.as_ptr(), texture) };
    }

    pub(crate) fn remove(&mut self, texture: *mut c_void) {
        // SAFETY: the allocation is a current member and remains alive through the following commit.
        unsafe { ck_mc_residency_remove(self.handle.as_ptr(), texture) };
    }

    pub(crate) fn commit(&mut self) {
        // SAFETY: the set remains attached to its retained context queue.
        unsafe { ck_mc_residency_commit(self.handle.as_ptr()) };
    }

    pub(crate) fn allocated_size(&self) -> u64 {
        // SAFETY: synchronous read of this live residency set.
        unsafe { ck_mc_residency_size(self.handle.as_ptr()) }
    }

    pub(crate) fn commit_count(&self) -> u64 {
        // SAFETY: synchronous read of this live residency set on its owner thread.
        unsafe { ck_mc_residency_commit_count(self.handle.as_ptr()) }
    }
}

impl ResourceTables {
    pub(crate) fn new(
        context: &MetalContext,
        slots: u32,
        budget: u64,
        overhead: u64,
    ) -> Result<Self, Error> {
        let requested = u64::from(slots)
            .checked_mul(16)
            .ok_or_else(|| Error("bindless table byte count overflow".to_owned()))?;
        let total = requested
            .checked_add(overhead)
            .ok_or_else(|| Error("bindless table and instance byte count overflow".to_owned()))?;
        if total > budget {
            return Err(Error(format!(
                "bindless tables and instance buffer need {total} bytes (tables {requested}, instance {overhead}), over binding_geometry_bytes budget {budget}"
            )));
        }
        let mut error = [0_u8; 1024];
        let mut allocated = 0_u64;
        // SAFETY: the retained context and bounded output buffers live through the synchronous native allocation.
        let raw = unsafe {
            ck_mc_tables_new(
                context.raw(),
                slots,
                &raw mut allocated,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let handle = NonNull::new(raw).ok_or_else(|| crate::message(&error))?;
        // SAFETY: version zero returns a buffer borrowed from the live native table holder.
        let first = unsafe { ck_mc_tables_buffer(handle.as_ptr(), 0) };
        // SAFETY: version one returns a buffer borrowed from the live native table holder.
        let second = unsafe { ck_mc_tables_buffer(handle.as_ptr(), 1) };
        let buffers = [first, second];
        if buffers.iter().any(|buffer| buffer.is_null()) {
            // SAFETY: this holder was just created and is the unique owner of both native buffers.
            unsafe { ck_mc_tables_free(handle.as_ptr()) };
            return Err(Error(
                "bindless table creation returned a null table buffer".to_owned(),
            ));
        }
        let Some(measured_total) = allocated.checked_add(overhead) else {
            // SAFETY: the newly-created holder is the unique owner of both measured table buffers.
            unsafe { ck_mc_tables_free(handle.as_ptr()) };
            return Err(Error(
                "measured bindless table and instance byte count overflow".to_owned(),
            ));
        };
        if measured_total > budget {
            // SAFETY: the newly-created holder is the unique owner of both measured table buffers.
            unsafe { ck_mc_tables_free(handle.as_ptr()) };
            return Err(Error(format!(
                "measured bindless tables and instance buffer need {measured_total} bytes (tables {allocated}, instance {overhead}), over binding_geometry_bytes budget {budget}"
            )));
        }
        let count = usize::try_from(slots)
            .map_err(|_| Error("bindless slot count exceeds host address space".to_owned()))?;
        Ok(Self {
            handle,
            slots,
            buffers,
            allocated,
            current: 0,
            epoch: 0,
            version_epochs: [0; 2],
            wanted: vec![0; count],
            staged: vec![false; count],
            contents: [vec![0; count], vec![0; count]],
            dirty: [VecDeque::new(), VecDeque::new()],
            queued: [vec![false; count], vec![false; count]],
            changed: Vec::new(),
            changed_marked: vec![false; count],
        })
    }

    pub(crate) fn add_residency(&self, residency: &mut ResidencySet) {
        residency.add(self.buffers[0]);
        residency.add(self.buffers[1]);
    }

    pub(crate) fn remove_residency(&self, residency: &mut ResidencySet) {
        residency.remove(self.buffers[0]);
        residency.remove(self.buffers[1]);
    }

    pub(crate) fn allocated_size(&self) -> u64 {
        self.allocated
    }

    pub(crate) fn initialize_checker(&mut self, resource_id: u64) -> Result<(), Error> {
        for (slot, wanted) in self.wanted.iter_mut().enumerate() {
            if !self.staged[slot] {
                *wanted = resource_id;
            }
        }
        for version in 0..2 {
            for slot in 0..self.slots {
                self.write(version, slot, resource_id)?;
            }
        }
        let spare = self.current ^ 1;
        for slot in 0..self.slots {
            let index = slot as usize;
            if self.staged[index]
                && self.contents[spare][index] != self.wanted[index]
                && !self.queued[spare][index]
            {
                self.queued[spare][index] = true;
                self.dirty[spare].push_back(slot);
            }
        }
        Ok(())
    }

    pub(crate) fn set_wanted(&mut self, slot: u32, resource_id: u64) {
        let Some(wanted) = self.wanted.get_mut(slot as usize) else {
            return;
        };
        if *wanted == resource_id {
            return;
        }
        *wanted = resource_id;
        for version in 0..2 {
            if self.contents[version][slot as usize] != resource_id
                && !self.queued[version][slot as usize]
            {
                self.queued[version][slot as usize] = true;
                self.dirty[version].push_back(slot);
            }
        }
    }

    /// Prepares a new revision in the spare table while the current table keeps its last published value.
    pub(crate) fn stage(&mut self, slot: u32, resource_id: u64) {
        let Some(wanted) = self.wanted.get_mut(slot as usize) else {
            return;
        };
        self.staged[slot as usize] = true;
        *wanted = resource_id;
        let spare = self.current ^ 1;
        if self.contents[spare][slot as usize] != resource_id && !self.queued[spare][slot as usize]
        {
            self.queued[spare][slot as usize] = true;
            self.dirty[spare].push_back(slot);
        }
        if !self.changed_marked[slot as usize] {
            self.changed_marked[slot as usize] = true;
            self.changed.push(slot);
        }
    }

    /// Sanitises a retired slot in both versions, once each version is writable.
    pub(crate) fn sanitize(&mut self, slot: u32, checker_id: u64) {
        self.set_wanted(slot, checker_id);
        if !self.changed_marked[slot as usize] {
            self.changed_marked[slot as usize] = true;
            self.changed.push(slot);
        }
    }

    pub(crate) fn catch_up(&mut self, pins: [usize; 2]) -> Result<usize, Error> {
        let mut total = 0;
        for (version, pin_count) in pins.iter().enumerate() {
            if *pin_count != 0 {
                continue;
            }
            for _ in 0..TABLE_WRITES_PER_PASS {
                let Some(slot) = self.dirty[version].pop_front() else {
                    break;
                };
                self.queued[version][slot as usize] = false;
                let value = self.wanted[slot as usize];
                self.write(version, slot, value)?;
                total += 1;
            }
        }
        Ok(total)
    }

    pub(crate) fn publish_if_ready(&mut self, pins: [usize; 2], requested: bool) -> bool {
        let spare = self.current ^ 1;
        if requested && pins[spare] == 0 && self.dirty[spare].is_empty() {
            self.current = spare;
            self.epoch = self.epoch.saturating_add(1);
            self.version_epochs[spare] = self.epoch;
            let old = spare ^ 1;
            for slot in self.changed.drain(..) {
                self.changed_marked[slot as usize] = false;
                self.staged[slot as usize] = false;
                if self.contents[old][slot as usize] != self.wanted[slot as usize]
                    && !self.queued[old][slot as usize]
                {
                    self.queued[old][slot as usize] = true;
                    self.dirty[old].push_back(slot);
                }
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn view(&self, version: usize) -> TableView {
        TableView {
            buffer: self.buffers[version],
            epoch: self.version_epochs[version],
            slots: self.slots,
        }
    }

    pub(crate) fn current(&self) -> usize {
        self.current
    }
    pub(crate) fn contents(&self, version: usize, slot: u32) -> Option<u64> {
        self.contents.get(version)?.get(slot as usize).copied()
    }
    pub(crate) fn has_dirty(&self) -> bool {
        self.dirty.iter().any(|slots| !slots.is_empty())
    }

    pub(crate) fn clear_staged(&mut self, slot: u32) {
        if let Some(staged) = self.staged.get_mut(slot as usize) {
            *staged = false;
        }
    }

    fn write(&mut self, version: usize, slot: u32, value: u64) -> Result<(), Error> {
        let native_version = u32::try_from(version)
            .map_err(|_| Error("bindless table version exceeds native range".to_owned()))?;
        // SAFETY: native holder is live, and version/slot bounds are checked by this state.
        let result =
            unsafe { ck_mc_tables_write(self.handle.as_ptr(), native_version, slot, value) };
        if result != 0 {
            return Err(Error("native bindless table write was refused".to_owned()));
        }
        self.contents[version][slot as usize] = value;
        Ok(())
    }
}

impl Drop for ResourceTables {
    fn drop(&mut self) {
        // SAFETY: unique Rust owner releases the native table holder exactly once.
        unsafe { ck_mc_tables_free(self.handle.as_ptr()) };
    }
}

impl Drop for ResidencySet {
    fn drop(&mut self) {
        // SAFETY: the native releaser detaches the set before releasing it; the native holder retains its context.
        unsafe { ck_mc_residency_free(self.handle.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BindlessLimits, MembershipBudget, RESIDENCY_CHANGES_PER_PASS, ResidencySet, ResourceSlots,
        ResourceTables, TABLE_WRITES_PER_PASS,
    };
    use crate::MetalContext;

    fn limits(slots: u32, textures: u32, extra: u32) -> BindlessLimits {
        BindlessLimits {
            texture_slots: slots,
            max_textures: textures,
            max_extra_revisions: extra,
        }
    }

    #[test]
    fn capacity_gate_checks_range_and_overflow() {
        assert!(limits(2, 1, 0).validate().is_ok());
        assert!(
            limits(1, 0, 0)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("outside 2..=1000000")
        );
        assert!(limits(1_000_001, 0, 0).validate().is_err());
        assert!(limits(6, 4, 1).validate().is_ok());
        assert!(
            limits(5, 4, 2)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("required 7")
        );
        assert!(
            limits(10, u32::MAX, 1)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
    }

    #[test]
    fn resource_slots_reserve_zero_and_never_exceed_the_configured_count() {
        let mut slots = ResourceSlots::new(3);
        assert_eq!(slots.allocate(), Some(1));
        assert_eq!(slots.allocate(), Some(2));
        assert_eq!(slots.allocate(), None);
        slots.release(1);
        assert_eq!(slots.allocate(), Some(1));
        assert_eq!(slots.allocate(), None);
    }

    #[test]
    fn residency_membership_work_is_bounded_per_pass() {
        let mut budget = MembershipBudget::new(RESIDENCY_CHANGES_PER_PASS);
        for _ in 0..1_024 {
            assert!(budget.take());
        }
        assert_eq!(budget.remaining(), 0);
        assert!(!budget.take());
    }

    #[test]
    fn table_budget_accounts_for_both_versions_and_instance_reservation() {
        let context = MetalContext::new().expect("Metal device");
        let error = match ResourceTables::new(&context, 8, 128, 1) {
            Ok(_) => panic!("both tables and the instance reservation exceed the budget"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("need 129 bytes"), "{error}");
        assert!(error.contains("tables 128, instance 1"), "{error}");
        assert!(error.contains("budget 128"), "{error}");
    }

    #[test]
    fn tier_one_refusal_precedes_native_allocation() {
        let context = MetalContext::new().expect("Metal device");
        let before = context.native_live_objects();
        let error = ResidencySet::start(&context, limits(4, 1, 1), true, false)
            .err()
            .expect("Tier 1 refusal");
        assert!(error.to_string().contains("argumentBuffersSupport"));
        assert!(error.to_string().contains("requested Tier 2"));
        assert!(error.to_string().contains("available Tier"));
        let after = context.native_live_objects();
        assert_eq!(after.textures, before.textures);
        assert_eq!(after.residency_sets, before.residency_sets);
        assert_eq!(after.residency_members, before.residency_members);
    }

    #[test]
    fn native_residency_creation_refusal_is_reported() {
        let context = MetalContext::new().expect("Metal device");
        let before = context.native_live_objects();
        let error = ResidencySet::start(&context, limits(4, 1, 1), false, true)
            .err()
            .expect("injected residency creation refusal");
        assert!(error.to_string().contains("MTLResidencySet"));
        assert!(error.to_string().contains("requested capacity 7"));
        assert!(error.to_string().contains("available 0"));
        let after = context.native_live_objects();
        assert_eq!(after.textures, before.textures);
        assert_eq!(after.residency_sets, before.residency_sets);
        assert_eq!(after.residency_members, before.residency_members);
    }

    #[test]
    fn invalid_capacity_is_refused_before_residency_creation() {
        let context = MetalContext::new().expect("Metal device");
        let before = context.native_live_objects();
        let error = ResidencySet::start(&context, limits(5, 4, 2), false, false)
            .err()
            .expect("insufficient slot capacity");
        assert!(error.to_string().contains("texture_slots=5"));
        assert!(error.to_string().contains("required 7"));
        let after = context.native_live_objects();
        assert_eq!(after.residency_sets, before.residency_sets);
        assert_eq!(after.residency_members, before.residency_members);
    }

    #[test]
    fn table_catch_up_is_bounded_and_keeps_the_pinned_current_epoch_unchanged() {
        let context = MetalContext::new().expect("Metal device");
        let slots = 10_001;
        let mut tables = ResourceTables::new(&context, slots, 2 * 1024 * 1024, 0)
            .expect("10,001 slot tables fit the test budget");
        let checker = 0x1122_3344_5566_7788;
        let staged = 0x8877_6655_4433_2211;
        tables.stage(1, staged);
        tables
            .initialize_checker(checker)
            .expect("initialize both table versions");
        assert_eq!(
            tables.wanted[1], staged,
            "checker init preserves staged IDs"
        );
        tables.clear_staged(1);
        tables
            .initialize_checker(checker)
            .expect("reinitialize a released slot to the checker");
        assert_eq!(
            tables.wanted[1], checker,
            "released slots return to checker"
        );
        let old_epoch = tables.view(0).epoch;
        for slot in 1..slots {
            tables.stage(slot, u64::from(slot) + 0x1000);
        }

        for pass in 0..3 {
            let expected = if pass < 2 {
                TABLE_WRITES_PER_PASS
            } else {
                1_808
            };
            assert_eq!(
                tables.catch_up([1, 0]).expect("bounded spare update"),
                expected
            );
            assert_eq!(tables.current(), 0);
            assert_eq!(tables.contents(0, 1), Some(checker));
            if pass < 2 {
                assert!(!tables.publish_if_ready([1, 0], true));
            }
        }
        assert!(tables.publish_if_ready([1, 0], true));
        assert_eq!(tables.current(), 1);
        assert_eq!(tables.view(0).epoch, old_epoch);
        assert_eq!(tables.view(1).epoch, old_epoch + 1);
        assert_eq!(tables.contents(1, 10_000), Some(0x1000 + 10_000));

        // Version zero is the old epoch pinned through publication; its entries stay unchanged
        // until that reader token ends, then its own dirty queue also drains in three passes.
        assert_eq!(
            tables
                .catch_up([1, 0])
                .expect("pinned old table is skipped"),
            0
        );
        assert_eq!(tables.contents(0, 10_000), Some(checker));
        for pass in 0..3 {
            let expected = if pass < 2 {
                TABLE_WRITES_PER_PASS
            } else {
                1_808
            };
            assert_eq!(
                tables.catch_up([0, 0]).expect("old table catch-up"),
                expected
            );
        }
        for slot in [1, 4_096, 10_000] {
            let value = u64::from(slot) + 0x1000;
            assert_eq!(tables.contents(0, slot), Some(value));
            assert_eq!(tables.contents(1, slot), Some(value));
        }
    }

    #[test]
    fn checker_initialization_requeues_a_prepared_slot_caught_up_before_binding() {
        let context = MetalContext::new().expect("Metal device");
        let mut tables =
            ResourceTables::new(&context, 8, 1024, 0).expect("eight table slots fit the budget");
        let checker = 0x1122_3344_5566_7788;
        let staged = 0x8877_6655_4433_2211;
        tables.stage(1, staged);
        assert_eq!(tables.catch_up([0, 0]).expect("pre-bind catch-up"), 1);
        assert_eq!(tables.contents(1, 1), Some(staged));

        tables
            .initialize_checker(checker)
            .expect("initialize checker after the staged write drained");
        assert_eq!(tables.wanted[1], staged);
        assert!(tables.has_dirty(), "staged entry must be queued again");
        assert_eq!(tables.catch_up([0, 0]).expect("post-bind catch-up"), 1);
        assert!(tables.publish_if_ready([0, 0], true));
        assert_eq!(tables.contents(tables.current(), 1), Some(staged));
    }

    #[test]
    fn overwriting_a_staged_id_is_a_non_adoptable_negative_control() {
        let context = MetalContext::new().expect("Metal device");
        let mut tables =
            ResourceTables::new(&context, 8, 1024, 0).expect("eight table slots fit the budget");
        let checker = 0x1122_3344_5566_7788;
        let staged = 0x8877_6655_4433_2211;
        let slot = 1;
        tables.stage(slot, staged);

        // Negative control: this is the old `wanted.fill(checker)` behavior from initialization.
        tables.wanted.fill(checker);
        tables
            .initialize_checker(checker)
            .expect("initialize checker after the simulated overwrite");
        tables
            .catch_up([0, 0])
            .expect("catch up the queued staged slot");
        assert!(tables.publish_if_ready([0, 0], true));
        assert_eq!(tables.contents(tables.current(), slot), Some(checker));
        assert_ne!(
            tables.contents(tables.current(), slot),
            Some(staged),
            "adopt's resource-ID guard must keep the staged revision waiting"
        );
    }
}
