//! The bindless tables' pure book (milestone 5.5, issue 5; design §13.1, §13.3): which sampled-image slot holds which view
//! in each of the two table versions, which entries still have to be written, and which version a frame may bind. No Vulkan
//! object is touched here, so every rule is tested without a device; the ledger (`revisions.rs`) does the writes.
//!
//! - **Entries.** A table entry is `0`, the checker's view, or a raw image-view handle. Slot 0 is the checker's for the whole
//!   run; the other slots go to native revisions from a free list and never equal logical slots (§13.1).
//! - **Wanted and contents.** `wanted[slot]` is what the slot should name; `contents[version][slot]` is what the version
//!   names now. A change of a wanted entry (a revision's view, or a sanitisation back to the checker) marks the slot dirty in
//!   **both** versions, in a FIFO bounded by the slot count (with a membership flag, so a slot is queued once). A pass takes
//!   at most [`WRITES_PER_PASS`] entries of a writable version: O(entries applied), never a scan of the table (CK-148).
//!   An entry whose wanted value came back to what the version already names is skipped when it is taken.
//! - **Writability (§13.1).** A version is writable only when no ticket pins it. `begin_frame` pins the current version;
//!   a submitted ticket keeps the pin until its recorded proof; `discarded`/`not_submitted` unpin at once.
//! - **Needed slots and the bindable rule.** A slot is *needed* while a frame can resolve it: an Active revision's, the held
//!   compatibility revision's. A version is *bindable* when no needed slot is unapplied in it (its content differs from the
//!   wanted entry). The counts are kept incrementally, so the test costs nothing per frame. A revision that is only Prepared,
//!   or only awaiting sanitisation, is harmless: no mapping names its slot.
//! - **The swap rule.** At a boundary with no building ticket the other version becomes current (epoch + 1) only when the
//!   current one is pinned (so it cannot be written), the other is bindable, and the other has applied entries the current one
//!   has not. Otherwise the current version stays current and, if it is unpinned, is written in place.

use std::collections::VecDeque;

/// Entries one pass applies to one writable version (CK-127's bound).
pub(crate) const WRITES_PER_PASS: usize = 4_096;

/// The table value of "the checker's view".
pub(crate) const CHECKER: u64 = 0;

/// The slots and the two versions' entries.
#[derive(Debug)]
pub(crate) struct TableBook {
    slots: u32,
    free: Vec<u32>,
    wanted: Vec<u64>,
    needed: Vec<bool>,
    contents: [Vec<u64>; 2],
    queued: [Vec<bool>; 2],
    dirty: [VecDeque<u32>; 2],
    pins: [usize; 2],
    current: usize,
    epoch: u64,
    version_epochs: [u64; 2],
    ready: bool,
    /// Needed slots whose entry differs from the wanted one, per version.
    needed_unapplied: [usize; 2],
    /// `lead[v]`: slots the other version has not applied that `v` has.
    lead: [usize; 2],
}

/// What a boundary decided about the version the next frame binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Boundary {
    pub(crate) version: usize,
    pub(crate) epoch: u64,
    pub(crate) swapped: bool,
}

impl TableBook {
    /// A book of `slots` entries, all naming the checker in both versions (not yet written to the device: see
    /// [`mark_ready`](Self::mark_ready)).
    pub(crate) fn new(slots: u32) -> Self {
        let count = usize::try_from(slots).unwrap_or(0);
        Self {
            slots,
            free: (1..slots).rev().collect(),
            wanted: vec![CHECKER; count],
            needed: vec![false; count],
            contents: [vec![CHECKER; count], vec![CHECKER; count]],
            queued: [vec![false; count], vec![false; count]],
            dirty: [VecDeque::new(), VecDeque::new()],
            pins: [0; 2],
            current: 0,
            epoch: 0,
            version_epochs: [0; 2],
            ready: false,
            needed_unapplied: [0; 2],
            lead: [0; 2],
        }
    }

    pub(crate) fn slots(&self) -> u32 {
        self.slots
    }

    /// The tables were initialised to the checker on the device: writes and adoption may begin.
    pub(crate) fn mark_ready(&mut self) {
        self.ready = true;
    }

    // ---- slots ----

    /// A free native slot (never 0), or the reason none is: the slots in use (the checker's included) and the configured
    /// count.
    pub(crate) fn allocate(&mut self) -> Result<u32, String> {
        self.free.pop().ok_or_else(|| {
            format!(
                "bindless texture slots exhausted ({} in use, {} configured)",
                self.slots, self.slots
            )
        })
    }

    /// Slots in use, the checker's included.
    pub(crate) fn in_use(&self) -> usize {
        usize::try_from(self.slots)
            .unwrap_or(usize::MAX)
            .saturating_sub(self.free.len())
    }

    /// Returns a slot to the free list. It must name the checker in both versions and be wanted as such: a reused slot holds
    /// the checker until its new revision's entry is applied.
    pub(crate) fn free_slot(&mut self, slot: u32) {
        let index = slot as usize;
        debug_assert_ne!(slot, 0, "the checker's slot is never freed");
        debug_assert!(
            self.is_clear(slot),
            "a freed slot names the checker in both versions"
        );
        debug_assert_eq!(self.wanted[index], CHECKER);
        debug_assert!(!self.needed[index]);
        self.free.push(slot);
    }

    // ---- entries ----

    /// What the slot should name from now on (a revision's view, or [`CHECKER`]).
    pub(crate) fn set_wanted(&mut self, slot: u32, value: u64) {
        let index = slot as usize;
        if self.wanted[index] == value {
            return;
        }
        self.retally(slot, |book| book.wanted[index] = value);
        for version in 0..2 {
            if book_differs(&self.contents[version], index, value) && !self.queued[version][index] {
                self.queued[version][index] = true;
                self.dirty[version].push_back(slot);
            }
        }
    }

    /// Whether a frame can resolve the slot (Active, or held for the compatibility lane).
    pub(crate) fn set_needed(&mut self, slot: u32, needed: bool) {
        let index = slot as usize;
        if self.needed[index] != needed {
            self.retally(slot, |book| book.needed[index] = needed);
        }
    }

    pub(crate) fn wanted(&self, slot: u32) -> u64 {
        self.wanted[slot as usize]
    }

    /// What `version` names at `slot` now.
    pub(crate) fn contents(&self, version: usize, slot: u32) -> u64 {
        self.contents[version][slot as usize]
    }

    /// Whether `version` names the wanted entry of `slot`.
    pub(crate) fn applied(&self, version: usize, slot: u32) -> bool {
        self.ready && self.contents[version][slot as usize] == self.wanted[slot as usize]
    }

    /// Whether both versions name the checker at `slot`: no version names a view the slot used to hold.
    pub(crate) fn is_clear(&self, slot: u32) -> bool {
        self.contents[0][slot as usize] == CHECKER && self.contents[1][slot as usize] == CHECKER
    }

    // ---- pins and writes ----

    pub(crate) fn pin(&mut self, version: usize) {
        self.pins[version] += 1;
    }

    pub(crate) fn unpin(&mut self, version: usize) {
        self.pins[version] = self.pins[version].saturating_sub(1);
    }

    pub(crate) fn pins(&self, version: usize) -> usize {
        self.pins[version]
    }

    /// A version is writable only when no ticket pins it.
    pub(crate) fn writable(&self, version: usize) -> bool {
        self.ready && self.pins[version] == 0
    }

    /// Takes up to `budget` queued entries of `version` into `out` as `(slot, value)` writes, and records them as the
    /// version's contents. Nothing is taken from a pinned version. An entry whose version already names the wanted value is
    /// dropped without a write. Returns the number of entries taken from the queue.
    pub(crate) fn take_writes(
        &mut self,
        version: usize,
        budget: usize,
        out: &mut Vec<(u32, u64)>,
    ) -> usize {
        if !self.writable(version) {
            return 0;
        }
        let mut taken = 0;
        while taken < budget {
            let Some(slot) = self.dirty[version].pop_front() else {
                break;
            };
            taken += 1;
            let index = slot as usize;
            self.queued[version][index] = false;
            let value = self.wanted[index];
            if self.contents[version][index] != value {
                self.retally(slot, |book| book.contents[version][index] = value);
                out.push((slot, value));
            }
        }
        taken
    }

    /// Whether some writable version still has queued entries (runnable work for another pass).
    pub(crate) fn has_runnable(&self) -> bool {
        (0..2).any(|version| self.writable(version) && !self.dirty[version].is_empty())
    }

    /// Entries queued for `version` (including ones a pass would drop).
    #[cfg(test)]
    pub(crate) fn queued(&self, version: usize) -> usize {
        self.dirty[version].len()
    }

    // ---- versions ----

    pub(crate) fn current(&self) -> usize {
        self.current
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    #[cfg(test)]
    pub(crate) fn version_epoch(&self, version: usize) -> u64 {
        self.version_epochs[version]
    }

    /// No needed slot is unapplied in `version`: a frame may bind it.
    pub(crate) fn bindable(&self, version: usize) -> bool {
        self.ready && self.needed_unapplied[version] == 0
    }

    /// The boundary's choice of the version the next frame binds (no ticket is building). See the module documentation.
    pub(crate) fn boundary(&mut self) -> Boundary {
        let current = self.current;
        let spare = current ^ 1;
        let swap =
            self.ready && self.pins[current] != 0 && self.bindable(spare) && self.lead[spare] != 0;
        if swap {
            self.current = spare;
            self.epoch = self.epoch.saturating_add(1);
            self.version_epochs[spare] = self.epoch;
        }
        Boundary {
            version: self.current,
            epoch: self.epoch,
            swapped: swap,
        }
    }

    // ---- the counts ----

    /// The slot's share in the two counts: `[needed_unapplied, lead]`, each per version.
    fn tally(&self, slot: u32) -> ([bool; 2], [bool; 2]) {
        let index = slot as usize;
        let unapplied = [
            self.contents[0][index] != self.wanted[index],
            self.contents[1][index] != self.wanted[index],
        ];
        (
            [
                self.needed[index] && unapplied[0],
                self.needed[index] && unapplied[1],
            ],
            [unapplied[1] && !unapplied[0], unapplied[0] && !unapplied[1]],
        )
    }

    /// Runs `change` on the slot's state and keeps the counts in step with it.
    fn retally(&mut self, slot: u32, change: impl FnOnce(&mut Self)) {
        let (needed_before, lead_before) = self.tally(slot);
        change(self);
        let (needed_after, lead_after) = self.tally(slot);
        for version in 0..2 {
            self.needed_unapplied[version] = adjust(
                self.needed_unapplied[version],
                needed_before[version],
                needed_after[version],
            );
            self.lead[version] = adjust(
                self.lead[version],
                lead_before[version],
                lead_after[version],
            );
        }
    }

    #[cfg(test)]
    fn recount(&self) -> ([usize; 2], [usize; 2]) {
        let mut needed = [0; 2];
        let mut lead = [0; 2];
        for slot in 0..self.slots {
            let (n, l) = self.tally(slot);
            for version in 0..2 {
                needed[version] += usize::from(n[version]);
                lead[version] += usize::from(l[version]);
            }
        }
        (needed, lead)
    }
}

fn book_differs(contents: &[u64], index: usize, value: u64) -> bool {
    contents[index] != value
}

fn adjust(count: usize, before: bool, after: bool) -> usize {
    match (before, after) {
        (false, true) => count + 1,
        (true, false) => count.saturating_sub(1),
        _ => count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(slots: u32) -> TableBook {
        let mut book = TableBook::new(slots);
        book.mark_ready();
        book
    }

    /// Applies every queued entry of every writable version, returning the writes.
    fn flush(book: &mut TableBook) -> Vec<(usize, u32, u64)> {
        let mut all = Vec::new();
        for version in 0..2 {
            let mut writes = Vec::new();
            while book.take_writes(version, WRITES_PER_PASS, &mut writes) != 0 {}
            all.extend(
                writes
                    .into_iter()
                    .map(|(slot, value)| (version, slot, value)),
            );
        }
        all
    }

    #[test]
    fn both_versions_start_at_the_checker_and_nothing_is_queued() {
        let book = ready(8);
        for version in 0..2 {
            for slot in 0..8 {
                assert_eq!(book.contents(version, slot), CHECKER);
                assert!(book.applied(version, slot));
            }
            assert_eq!(book.queued(version), 0);
            assert!(book.bindable(version));
        }
        assert_eq!((book.current(), book.epoch()), (0, 0));
        assert!(!book.has_runnable());
    }

    #[test]
    fn a_wanted_change_marks_the_slot_dirty_in_both_versions_once() {
        let mut book = ready(8);
        let slot = book.allocate().unwrap();
        book.set_wanted(slot, 0xAA);
        book.set_wanted(slot, 0xBB);
        assert_eq!(
            (book.queued(0), book.queued(1)),
            (1, 1),
            "queued once per version"
        );
        let writes = flush(&mut book);
        assert_eq!(
            writes,
            vec![(0, slot, 0xBB), (1, slot, 0xBB)],
            "the latest wanted value"
        );
        assert!(!book.has_runnable());
        // Back to what the versions name before a pass: queued, but dropped without a write.
        book.set_wanted(slot, 0xCC);
        book.set_wanted(slot, 0xBB);
        assert_eq!(book.queued(0), 1);
        assert!(
            flush(&mut book).is_empty(),
            "nothing to write: the versions already name 0xBB"
        );
        assert_eq!(book.queued(0), 0);
    }

    #[test]
    fn ten_thousand_dirty_slots_take_three_passes_per_version() {
        let mut book = ready(10_001);
        for _ in 0..10_000 {
            let slot = book.allocate().unwrap();
            book.set_wanted(slot, u64::from(slot) + 100);
        }
        for version in 0..2 {
            let mut passes = 0;
            let mut written = 0;
            loop {
                let mut out = Vec::new();
                let taken = book.take_writes(version, WRITES_PER_PASS, &mut out);
                if taken == 0 {
                    break;
                }
                assert!(taken <= WRITES_PER_PASS);
                passes += 1;
                written += out.len();
            }
            assert_eq!((passes, written), (3, 10_000), "version {version}");
        }
        assert!(!book.has_runnable());
    }

    #[test]
    fn a_pinned_version_is_never_written_and_an_unpin_makes_it_writable_at_once() {
        let mut book = ready(8);
        let slot = book.allocate().unwrap();
        book.pin(0);
        book.set_wanted(slot, 0xAA);
        let mut out = Vec::new();
        assert_eq!(book.take_writes(0, WRITES_PER_PASS, &mut out), 0);
        assert!(out.is_empty());
        assert_eq!(
            book.contents(0, slot),
            CHECKER,
            "the pinned version still names the checker"
        );
        assert_eq!(
            book.take_writes(1, WRITES_PER_PASS, &mut out),
            1,
            "the other version is written"
        );
        assert_eq!(book.contents(1, slot), 0xAA);
        book.pin(0);
        book.unpin(0);
        assert!(!book.writable(0), "two pins, one unpin: still pinned");
        book.unpin(0);
        assert!(book.writable(0));
        assert_eq!(book.take_writes(0, WRITES_PER_PASS, &mut out), 1);
        assert_eq!(book.contents(0, slot), 0xAA);
    }

    #[test]
    fn exhaustion_names_the_slots_and_leaves_the_book_unchanged() {
        let mut book = ready(3);
        assert_eq!(book.in_use(), 1, "the checker's slot");
        let first = book.allocate().unwrap();
        let second = book.allocate().unwrap();
        assert_ne!(first, 0);
        assert_ne!(second, 0);
        assert_ne!(first, second);
        let before = (book.in_use(), book.queued(0), book.queued(1));
        assert_eq!(
            book.allocate().unwrap_err(),
            "bindless texture slots exhausted (3 in use, 3 configured)"
        );
        assert_eq!(before, (book.in_use(), book.queued(0), book.queued(1)));
        book.free_slot(first);
        assert_eq!(book.allocate().unwrap(), first, "a freed slot is reused");
    }

    #[test]
    fn a_slot_is_clear_only_when_both_versions_name_the_checker() {
        let mut book = ready(4);
        let slot = book.allocate().unwrap();
        book.set_wanted(slot, 0xAA);
        flush(&mut book);
        assert!(!book.is_clear(slot));
        book.set_wanted(slot, CHECKER);
        let mut out = Vec::new();
        book.take_writes(0, WRITES_PER_PASS, &mut out);
        assert!(!book.is_clear(slot), "only version 0 is sanitised");
        book.take_writes(1, WRITES_PER_PASS, &mut out);
        assert!(book.is_clear(slot));
        book.free_slot(slot);
    }

    #[test]
    fn the_bindable_rule_counts_needed_slots_only() {
        let mut book = ready(8);
        let slot = book.allocate().unwrap();
        book.set_wanted(slot, 0xAA);
        assert!(
            book.bindable(0) && book.bindable(1),
            "a prepared revision is not needed yet"
        );
        book.pin(1);
        let mut out = Vec::new();
        book.take_writes(0, WRITES_PER_PASS, &mut out);
        book.set_needed(slot, true);
        assert!(book.bindable(0), "version 0 applied the needed slot");
        assert!(!book.bindable(1), "version 1 did not");
        book.unpin(1);
        book.take_writes(1, WRITES_PER_PASS, &mut out);
        assert!(book.bindable(1));
        // Sanitising a slot that is no longer needed does not make a version unbindable.
        book.set_needed(slot, false);
        book.set_wanted(slot, CHECKER);
        assert!(book.bindable(0) && book.bindable(1));
        assert_eq!(book.recount(), (book.needed_unapplied, book.lead));
    }

    #[test]
    fn the_swap_rule_makes_progress_under_continuous_frames() {
        // The worst case the protocol allows: the previous frame is still pending (it pins its version) when the next
        // boundary runs, and a new publication arrives at every boundary.
        let mut book = ready(64);
        let mut pending: Option<usize> = None;
        let mut shown: Option<u32> = None;
        let mut retired: Vec<u32> = Vec::new();
        let mut swaps = 0;
        for round in 0_u64..40 {
            let slot = book.allocate().unwrap();
            book.set_wanted(slot, 0x1000 + round);
            // A pass: every writable version takes its queue; the pinned one is left alone.
            let mut out = Vec::new();
            for version in 0..2 {
                if Some(version) == pending {
                    assert!(!book.writable(version));
                }
                book.take_writes(version, WRITES_PER_PASS, &mut out);
            }
            let boundary = book.boundary();
            swaps += usize::from(boundary.swapped);
            assert!(
                book.applied(boundary.version, slot),
                "round {round}: the publication is applied in the version the next frame binds"
            );
            assert!(
                book.bindable(boundary.version),
                "round {round}: the bound version is bindable"
            );
            // Adopt: the new revision is shown, the old one is retired and sanitised in both versions.
            book.set_needed(slot, true);
            if let Some(old) = shown.replace(slot) {
                book.set_needed(old, false);
                book.set_wanted(old, CHECKER);
                retired.push(old);
            }
            // The frame is built on this version and pins it; the previous frame's proof arrives now.
            book.pin(boundary.version);
            if let Some(previous) = pending.replace(boundary.version) {
                book.unpin(previous);
            }
            // A retired slot may be freed once both versions name the checker; none is freed while one still names it.
            let (clear, waiting): (Vec<u32>, Vec<u32>) =
                retired.drain(..).partition(|&slot| book.is_clear(slot));
            for slot in clear {
                book.free_slot(slot);
            }
            retired = waiting;
            assert!(
                retired.len() <= 3,
                "retired slots drain under continuous frames: {retired:?}"
            );
        }
        assert_eq!(
            swaps, 39,
            "every later boundary swaps: the spare is the only writable version"
        );
        assert_eq!(book.epoch(), 39);
        assert_eq!(book.recount(), (book.needed_unapplied, book.lead));
    }

    #[test]
    fn the_boundary_swaps_only_a_pinned_current_version_to_a_bindable_spare_that_leads() {
        let mut book = ready(8);
        let slot = book.allocate().unwrap();
        book.pin(0);
        book.set_wanted(slot, 0xAA);
        let mut out = Vec::new();
        book.take_writes(1, WRITES_PER_PASS, &mut out);
        let boundary = book.boundary();
        assert_eq!(
            boundary,
            Boundary {
                version: 1,
                epoch: 1,
                swapped: true
            },
            "version 1 leads and is bindable; version 0 is pinned"
        );
        assert_eq!(book.version_epoch(1), 1);
        // The same call again changes nothing (the new current version is unpinned and nothing leads it).
        assert!(!book.boundary().swapped);
        // An unpinned current version stays current and is written in place.
        let mut book = ready(8);
        let slot = book.allocate().unwrap();
        book.set_wanted(slot, 0xAA);
        book.take_writes(1, WRITES_PER_PASS, &mut out);
        assert!(!book.boundary().swapped, "version 0 is unpinned");
        book.take_writes(0, WRITES_PER_PASS, &mut out);
        assert!(book.applied(0, slot));
        // A spare that is not bindable is not swapped to.
        let mut book = ready(8);
        let slot = book.allocate().unwrap();
        book.set_wanted(slot, 0xAA);
        let mut sink = Vec::new();
        book.take_writes(0, WRITES_PER_PASS, &mut sink);
        book.take_writes(1, WRITES_PER_PASS, &mut sink);
        book.set_needed(slot, true);
        book.pin(0);
        book.set_wanted(slot, 0xBB);
        book.set_needed(slot, false);
        let other = book.allocate().unwrap();
        book.set_wanted(other, 0xCC);
        book.set_needed(other, true);
        assert!(
            !book.bindable(1),
            "the needed slot is unapplied in version 1"
        );
        assert!(!book.boundary().swapped);
    }

    #[test]
    fn nothing_is_written_or_adopted_before_the_tables_are_ready() {
        let mut book = TableBook::new(8);
        let slot = book.allocate().unwrap();
        book.set_wanted(slot, 0xAA);
        let mut out = Vec::new();
        assert_eq!(book.take_writes(0, WRITES_PER_PASS, &mut out), 0);
        assert!(!book.applied(0, slot) && !book.bindable(0) && !book.has_runnable());
        book.mark_ready();
        assert!(book.has_runnable());
        assert_eq!(book.take_writes(0, WRITES_PER_PASS, &mut out), 1);
    }

    #[test]
    fn the_counts_stay_equal_to_a_full_recount_under_random_operations() {
        // A small deterministic generator, so a failure replays.
        let mut state = 0x1234_5678_9abc_def0_u64;
        let mut next = move |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        let mut book = ready(32);
        let mut live: Vec<u32> = Vec::new();
        for _ in 0..20_000 {
            match next(8) {
                0 => {
                    if let Ok(slot) = book.allocate() {
                        live.push(slot);
                        book.set_wanted(slot, next(1 << 20) + 1);
                    }
                }
                1 if !live.is_empty() => {
                    let slot = live[usize::try_from(next(live.len() as u64)).unwrap()];
                    book.set_needed(slot, true);
                }
                2 if !live.is_empty() => {
                    let slot = live[usize::try_from(next(live.len() as u64)).unwrap()];
                    book.set_needed(slot, false);
                    book.set_wanted(slot, CHECKER);
                }
                3 => book.pin(usize::try_from(next(2)).unwrap()),
                4 => book.unpin(usize::try_from(next(2)).unwrap()),
                5 | 6 => {
                    let mut out = Vec::new();
                    book.take_writes(
                        usize::try_from(next(2)).unwrap(),
                        usize::try_from(next(5) + 1).unwrap(),
                        &mut out,
                    );
                }
                _ => {
                    book.boundary();
                }
            }
            assert_eq!(book.recount(), (book.needed_unapplied, book.lead));
        }
    }
}
