//! Pooled device memory for the texture store's images (design §7.2, the Vulkan proposal), as pure bookkeeping.
//!
//! The pool never touches Vulkan: it asks a [`MemorySource`] for whole blocks and dedicated allocations, so
//! the tests drive it with fake handles. The store gives it a source that allocates through the context, which
//! is also where the allocation count is kept.
//!
//! - **Segregated classes.** Only optimal-tiling, sampled images come from this pool. Linear resources (the
//!   staging buffers) never share a block with them, so `bufferImageGranularity` never applies inside a
//!   block.
//! - **Blocks.** One list per memory type index; each block is [`BLOCK_SIZE`] bytes and is suballocated
//!   first-fit over an offset-ordered free list, at the queried alignment. Adjacent free ranges coalesce on
//!   release, and a block whose last range is released is freed.
//! - **Dedicated allocations.** An image that requires a dedicated allocation, or that is larger than half a
//!   block, or that prefers one and is at least [`PREFER_DEDICATED_MIN`], has an allocation of its own, of the
//!   queried size.
//! - **`texture_bytes` is a hard ceiling.** A block or a dedicated allocation is reserved at its queried size
//!   *before* the source is asked for memory, and a request that cannot fit is refused without asking.
//! - **No reuse while live.** A range returns to the free list only through [`ImagePool::release`], which the
//!   store calls when the image is destroyed. Nothing here ever hands out a range that is still held.

use std::collections::HashMap;
use std::fmt;

/// The size of one pooled block: 64 MiB (a constant, not a setting).
pub(crate) const BLOCK_SIZE: u64 = 64 * 1024 * 1024;

/// The smallest image whose *preferred* dedicated allocation is honoured: a sixteenth of a block (4 MiB).
///
/// `prefersDedicatedAllocation` is a hint. A driver may set it on every image, including 16 x 16 ones (`MoltenVK`
/// does), and honouring it for those would make one `vkAllocateMemory` per image, which defeats the pool and
/// the allocation-count limit it exists to respect. An image that *requires* a dedicated allocation always gets
/// one, and so does an image larger than half a block.
pub(crate) const PREFER_DEDICATED_MIN: u64 = BLOCK_SIZE / 16;

/// Rounds `value` up to a multiple of `alignment` (`alignment` is not zero); `None` on overflow.
pub(crate) fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment.checked_sub(1)?)
        .map(|sum| sum / alignment * alignment)
}

/// Whether an image wants its own allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dedication {
    /// The driver has no preference.
    No,
    /// `prefersDedicatedAllocation`.
    Preferred,
    /// `requiresDedicatedAllocation`.
    Required,
}

/// What one image needs, as `vkGetImageMemoryRequirements2` reported it, with the memory type already chosen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request {
    pub(crate) size: u64,
    pub(crate) alignment: u64,
    pub(crate) memory_type: u32,
    pub(crate) dedication: Dedication,
    /// The image's raw handle, named in the dedicated allocation when it is one the driver asked for.
    pub(crate) image: u64,
}

/// Why a source could not allocate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceError {
    /// The allocation count is at its limit.
    Count { in_use: u32, limit: u32 },
    /// The device refused (or the test injected a refusal).
    Device(String),
}

/// Where whole allocations come from.
pub(crate) trait MemorySource {
    /// A handle to one allocation.
    type Handle: Copy + Eq + fmt::Debug;

    /// Allocates `size` bytes of `memory_type`; `dedicated` names the image that gets the whole allocation.
    fn allocate(
        &mut self,
        size: u64,
        memory_type: u32,
        dedicated: Option<u64>,
    ) -> Result<Self::Handle, SourceError>;

    /// Frees an allocation this source made.
    fn free(&mut self, handle: Self::Handle);
}

/// Why the pool refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AllocError {
    /// The allocation would take the reserved bytes over `texture_bytes`.
    Budget {
        needed: u64,
        reserved: u64,
        limit: u64,
    },
    /// The source refused.
    Source(SourceError),
}

/// A held range of an allocation. Handed back to [`ImagePool::release`] exactly once.
#[derive(Debug)]
pub(crate) struct Placement<H> {
    pub(crate) memory: H,
    pub(crate) offset: u64,
    pub(crate) size: u64,
    block: usize,
}

struct Block<H> {
    memory: H,
    size: u64,
    memory_type: u32,
    dedicated: bool,
    /// Free ranges `(offset, size)`, ordered by offset, never adjacent.
    free: Vec<(u64, u64)>,
    /// Bytes held by placements.
    used: u64,
}

/// The image memory of one store.
pub(crate) struct ImagePool<H> {
    limit: u64,
    reserved: u64,
    peak: u64,
    blocks: Vec<Option<Block<H>>>,
    vacant: Vec<usize>,
    /// The suballocated blocks of each memory type, in creation order.
    by_type: HashMap<u32, Vec<usize>>,
}

impl<H: Copy + Eq + fmt::Debug> ImagePool<H> {
    /// A pool that reserves at most `limit` bytes (`texture_bytes`).
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            limit,
            reserved: 0,
            peak: 0,
            blocks: Vec::new(),
            vacant: Vec::new(),
            by_type: HashMap::new(),
        }
    }

    /// The bytes reserved now: every live block and dedicated allocation, at its queried size.
    #[cfg(test)]
    pub(crate) fn reserved(&self) -> u64 {
        self.reserved
    }

    /// The most bytes ever reserved at once.
    pub(crate) fn peak(&self) -> u64 {
        self.peak
    }

    /// How many allocations (blocks and dedicated ones) are live.
    #[cfg(test)]
    pub(crate) fn allocations(&self) -> usize {
        self.blocks.iter().flatten().count()
    }

    /// Places an image.
    ///
    /// # Errors
    ///
    /// [`AllocError::Budget`] when a new block or dedicated allocation would pass the ceiling (the source is
    /// not asked), [`AllocError::Source`] when the source refuses (nothing is reserved then).
    pub(crate) fn allocate<S: MemorySource<Handle = H>>(
        &mut self,
        source: &mut S,
        request: &Request,
    ) -> Result<Placement<H>, AllocError> {
        let size = request.size.max(1);
        let alignment = request.alignment.max(1);
        let honoured = match request.dedication {
            Dedication::Required => true,
            Dedication::Preferred => size >= PREFER_DEDICATED_MIN,
            Dedication::No => false,
        };
        if honoured || size > BLOCK_SIZE / 2 {
            return self.allocate_dedicated(source, request, size);
        }
        if let Some(placement) = self.suballocate(request.memory_type, size, alignment) {
            return Ok(placement);
        }
        self.reserve(BLOCK_SIZE)?;
        let memory = match source.allocate(BLOCK_SIZE, request.memory_type, None) {
            Ok(memory) => memory,
            Err(error) => {
                self.reserved -= BLOCK_SIZE;
                return Err(AllocError::Source(error));
            }
        };
        let id = self.insert(Block {
            memory,
            size: BLOCK_SIZE,
            memory_type: request.memory_type,
            dedicated: false,
            free: vec![(0, BLOCK_SIZE)],
            used: 0,
        });
        self.by_type
            .entry(request.memory_type)
            .or_default()
            .push(id);
        // A fresh block starts at offset zero, which every alignment divides, and the request is at most half
        // of it: the first fit cannot fail.
        self.carve(id, size, alignment).ok_or_else(|| {
            AllocError::Source(SourceError::Device(
                "a fresh block could not hold the request".to_owned(),
            ))
        })
    }

    fn allocate_dedicated<S: MemorySource<Handle = H>>(
        &mut self,
        source: &mut S,
        request: &Request,
        size: u64,
    ) -> Result<Placement<H>, AllocError> {
        self.reserve(size)?;
        // The image is named only when the driver asked for a dedicated allocation and it is honoured; an image
        // that is merely too big for a block gets a plain allocation of its own.
        let image = match request.dedication {
            Dedication::Required => Some(request.image),
            Dedication::Preferred if size >= PREFER_DEDICATED_MIN => Some(request.image),
            _ => None,
        };
        let memory = match source.allocate(size, request.memory_type, image) {
            Ok(memory) => memory,
            Err(error) => {
                self.reserved -= size;
                return Err(AllocError::Source(error));
            }
        };
        let id = self.insert(Block {
            memory,
            size,
            memory_type: request.memory_type,
            dedicated: true,
            free: Vec::new(),
            used: size,
        });
        Ok(Placement {
            memory,
            offset: 0,
            size,
            block: id,
        })
    }

    /// Reserves `bytes` against the ceiling.
    fn reserve(&mut self, bytes: u64) -> Result<(), AllocError> {
        let total = self.reserved.checked_add(bytes);
        match total {
            Some(total) if total <= self.limit => {
                self.reserved = total;
                self.peak = self.peak.max(total);
                Ok(())
            }
            _ => Err(AllocError::Budget {
                needed: bytes,
                reserved: self.reserved,
                limit: self.limit,
            }),
        }
    }

    fn insert(&mut self, block: Block<H>) -> usize {
        if let Some(id) = self.vacant.pop() {
            self.blocks[id] = Some(block);
            id
        } else {
            self.blocks.push(Some(block));
            self.blocks.len() - 1
        }
    }

    /// First fit over the blocks of `memory_type`, in creation order.
    fn suballocate(&mut self, memory_type: u32, size: u64, alignment: u64) -> Option<Placement<H>> {
        let ids = self.by_type.get(&memory_type)?.clone();
        ids.into_iter()
            .find_map(|id| self.carve(id, size, alignment))
    }

    /// Takes `size` bytes at `alignment` from the first free range of block `id` that holds them.
    fn carve(&mut self, id: usize, size: u64, alignment: u64) -> Option<Placement<H>> {
        let block = self.blocks.get_mut(id)?.as_mut()?;
        let (index, start) =
            block
                .free
                .iter()
                .enumerate()
                .find_map(|(index, &(offset, length))| {
                    let start = align_up(offset, alignment)?;
                    let end = start.checked_add(size)?;
                    (end <= offset + length).then_some((index, start))
                })?;
        let (offset, length) = block.free[index];
        let end = start + size;
        let tail = (offset + length) - end;
        // The range leaves the list, and what the alignment and the size leave on either side stays in it.
        let mut leftovers = Vec::with_capacity(2);
        if start > offset {
            leftovers.push((offset, start - offset));
        }
        if tail > 0 {
            leftovers.push((end, tail));
        }
        block.free.splice(index..=index, leftovers);
        block.used += size;
        Some(Placement {
            memory: block.memory,
            offset: start,
            size,
            block: id,
        })
    }

    /// Returns a placement's range. A block that holds nothing any more is freed.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "taking the placement by value is what makes a range return exactly once"
    )]
    pub(crate) fn release<S: MemorySource<Handle = H>>(
        &mut self,
        source: &mut S,
        placement: Placement<H>,
    ) {
        let Placement {
            memory,
            offset,
            size,
            block: id,
        } = placement;
        let Some(block) = self.blocks.get_mut(id).and_then(Option::as_mut) else {
            debug_assert!(false, "a placement of a block that does not exist");
            return;
        };
        debug_assert_eq!(block.memory, memory);
        block.used = block.used.saturating_sub(size);
        if !block.dedicated {
            let at = block.free.partition_point(|&(start, _)| start < offset);
            debug_assert!(
                block
                    .free
                    .get(at)
                    .is_none_or(|&(start, _)| start >= offset + size),
                "a released range overlaps a free one"
            );
            block.free.insert(at, (offset, size));
            // Merge with the next range, then with the previous one.
            if at + 1 < block.free.len() {
                let (offset, length) = block.free[at];
                let (next_offset, next_length) = block.free[at + 1];
                if offset + length == next_offset {
                    block.free[at] = (offset, length + next_length);
                    block.free.remove(at + 1);
                }
            }
            if at > 0 {
                let (previous_offset, previous_length) = block.free[at - 1];
                let (offset, length) = block.free[at];
                if previous_offset + previous_length == offset {
                    block.free[at - 1] = (previous_offset, previous_length + length);
                    block.free.remove(at);
                }
            }
        }
        if block.used > 0 {
            return;
        }
        let (memory, bytes, memory_type, dedicated) =
            (block.memory, block.size, block.memory_type, block.dedicated);
        self.blocks[id] = None;
        self.vacant.push(id);
        if !dedicated && let Some(ids) = self.by_type.get_mut(&memory_type) {
            ids.retain(|held| *held != id);
            if ids.is_empty() {
                self.by_type.remove(&memory_type);
            }
        }
        self.reserved = self.reserved.saturating_sub(bytes);
        source.free(memory);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        AllocError, BLOCK_SIZE, Dedication, ImagePool, MemorySource, Placement, Request,
        SourceError, align_up,
    };

    const MIB: u64 = 1024 * 1024;

    /// Fake memory: counts allocations against a limit and remembers what is live.
    struct Fake {
        next: u64,
        live: HashSet<u64>,
        limit: u32,
        asked: Vec<(u64, u32, Option<u64>)>,
        fail: bool,
    }

    impl Fake {
        fn new(limit: u32) -> Self {
            Self {
                next: 1,
                live: HashSet::new(),
                limit,
                asked: Vec::new(),
                fail: false,
            }
        }
    }

    impl MemorySource for Fake {
        type Handle = u64;

        fn allocate(
            &mut self,
            size: u64,
            memory_type: u32,
            dedicated: Option<u64>,
        ) -> Result<u64, SourceError> {
            self.asked.push((size, memory_type, dedicated));
            if self.fail {
                return Err(SourceError::Device("refused".to_owned()));
            }
            let in_use = u32::try_from(self.live.len()).expect("a small count");
            if in_use >= self.limit {
                return Err(SourceError::Count {
                    in_use,
                    limit: self.limit,
                });
            }
            let handle = self.next;
            self.next += 1;
            self.live.insert(handle);
            Ok(handle)
        }

        fn free(&mut self, handle: u64) {
            assert!(self.live.remove(&handle), "freed twice or unknown");
        }
    }

    fn request(size: u64, alignment: u64) -> Request {
        Request {
            size,
            alignment,
            memory_type: 0,
            dedication: Dedication::No,
            image: 7,
        }
    }

    /// Whether any two placements of one memory overlap.
    fn overlap(placements: &[&Placement<u64>]) -> bool {
        placements.iter().enumerate().any(|(index, a)| {
            placements[index + 1..].iter().any(|b| {
                a.memory == b.memory && a.offset < b.offset + b.size && b.offset < a.offset + a.size
            })
        })
    }

    #[test]
    fn a_range_is_aligned_and_the_first_fit_takes_the_lowest_one() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(256 * MIB);
        let first = pool
            .allocate(&mut source, &request(1000, 256))
            .expect("fits");
        let second = pool
            .allocate(&mut source, &request(100, 4096))
            .expect("fits");
        assert_eq!(first.offset, 0);
        assert_eq!(second.offset, 4096, "aligned past the first range");
        let third = pool.allocate(&mut source, &request(64, 1)).expect("fits");
        assert_eq!(
            third.offset, 1000,
            "the hole before the aligned range is first fit"
        );
        assert!(!overlap(&[&first, &second, &third]));
        assert_eq!(pool.allocations(), 1);
        assert_eq!(pool.reserved(), BLOCK_SIZE);
    }

    #[test]
    fn released_neighbours_coalesce_and_the_empty_block_is_freed() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(256 * MIB);
        let a = pool.allocate(&mut source, &request(4096, 256)).expect("a");
        let b = pool.allocate(&mut source, &request(4096, 256)).expect("b");
        let c = pool.allocate(&mut source, &request(4096, 256)).expect("c");
        pool.release(&mut source, b);
        pool.release(&mut source, a);
        // a and b merged into one 8 KiB range that a larger request now takes whole.
        let d = pool.allocate(&mut source, &request(8192, 256)).expect("d");
        assert_eq!(d.offset, 0);
        assert!(!overlap(&[&c, &d]));
        pool.release(&mut source, c);
        assert_eq!(pool.allocations(), 1, "d still holds the block");
        pool.release(&mut source, d);
        assert_eq!(pool.allocations(), 0);
        assert_eq!(pool.reserved(), 0);
        assert!(
            source.live.is_empty(),
            "the empty block went back to the source"
        );
    }

    #[test]
    fn a_full_block_makes_a_second_one_and_each_memory_type_has_its_own_list() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(512 * MIB);
        let half = BLOCK_SIZE / 2;
        let a = pool.allocate(&mut source, &request(half, 1)).expect("a");
        let b = pool.allocate(&mut source, &request(half, 1)).expect("b");
        assert_eq!(a.memory, b.memory, "two halves fill one block");
        let c = pool.allocate(&mut source, &request(1, 1)).expect("c");
        assert_ne!(c.memory, a.memory, "the first block is full");
        let mut other = request(1, 1);
        other.memory_type = 3;
        let d = pool.allocate(&mut source, &other).expect("d");
        assert_ne!(d.memory, c.memory, "another type, another list");
        assert_eq!(pool.allocations(), 3);
        assert_eq!(source.asked.len(), 3);
        assert!(
            source
                .asked
                .iter()
                .all(|(size, _, image)| *size == BLOCK_SIZE && image.is_none())
        );
    }

    #[test]
    fn a_reservation_past_the_ceiling_is_refused_before_the_source_is_asked() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(64 * MIB);
        let first = pool
            .allocate(&mut source, &request(MIB, 1))
            .expect("one block fits");
        let mut other = request(1, 1);
        other.memory_type = 1;
        let refused = pool
            .allocate(&mut source, &other)
            .expect_err("a second block does not");
        assert_eq!(
            refused,
            AllocError::Budget {
                needed: BLOCK_SIZE,
                reserved: BLOCK_SIZE,
                limit: 64 * MIB
            }
        );
        assert_eq!(source.asked.len(), 1, "the source was not asked");
        assert_eq!(pool.reserved(), BLOCK_SIZE);
        // The held block still serves what fits in it.
        let second = pool
            .allocate(&mut source, &request(MIB, 1))
            .expect("room in the block");
        assert!(!overlap(&[&first, &second]));
    }

    #[test]
    fn dedicated_allocations_are_reserved_at_their_queried_size() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(512 * MIB);
        let mut required = request(10 * MIB, 1);
        required.dedication = Dedication::Required;
        let mut preferred = request(8 * MIB, 1);
        preferred.dedication = Dedication::Preferred;
        let big = request(BLOCK_SIZE / 2 + 1, 1);
        let a = pool.allocate(&mut source, &required).expect("required");
        let b = pool.allocate(&mut source, &preferred).expect("preferred");
        let c = pool.allocate(&mut source, &big).expect("over half a block");
        assert_eq!(pool.reserved(), 10 * MIB + 8 * MIB + BLOCK_SIZE / 2 + 1);
        assert_eq!(
            source.asked,
            vec![
                (10 * MIB, 0, Some(7)),
                (8 * MIB, 0, Some(7)),
                (BLOCK_SIZE / 2 + 1, 0, None)
            ],
            "the image is named when the driver asked for a dedicated allocation, not for a big one"
        );
        for placement in [&a, &b, &c] {
            assert_eq!(placement.offset, 0);
        }
        pool.release(&mut source, a);
        pool.release(&mut source, b);
        pool.release(&mut source, c);
        assert_eq!(pool.reserved(), 0);
        assert!(source.live.is_empty());
    }

    #[test]
    fn a_preferred_dedication_below_the_minimum_is_pooled_like_any_other_image() {
        // A driver that sets the hint on every image (MoltenVK) must not get one allocation per image.
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(512 * MIB);
        let mut hinted = request(1024, 4);
        hinted.dedication = Dedication::Preferred;
        let placements: Vec<_> = (0..50)
            .map(|_| pool.allocate(&mut source, &hinted).expect("room"))
            .collect();
        assert_eq!(
            source.asked.len(),
            1,
            "one block served fifty hinted images"
        );
        assert_eq!(pool.allocations(), 1);
        let refs: Vec<&Placement<u64>> = placements.iter().collect();
        assert!(!overlap(&refs));
        // The hint at the minimum is honoured, and the image is named.
        let mut large = request(super::PREFER_DEDICATED_MIN, 4);
        large.dedication = Dedication::Preferred;
        let own = pool.allocate(&mut source, &large).expect("own allocation");
        assert_eq!(source.asked[1], (super::PREFER_DEDICATED_MIN, 0, Some(7)));
        pool.release(&mut source, own);
        for placement in placements {
            pool.release(&mut source, placement);
        }
        assert!(source.live.is_empty());
    }

    #[test]
    fn a_dedicated_allocation_that_does_not_fit_the_ceiling_is_refused() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(64 * MIB);
        let mut required = request(48 * MIB, 1);
        required.dedication = Dedication::Required;
        let held = pool.allocate(&mut source, &required).expect("fits");
        let mut second = request(32 * MIB, 1);
        second.dedication = Dedication::Required;
        assert!(matches!(
            pool.allocate(&mut source, &second),
            Err(AllocError::Budget { needed, reserved, limit })
                if needed == 32 * MIB && reserved == 48 * MIB && limit == 64 * MIB
        ));
        pool.release(&mut source, held);
        pool.allocate(&mut source, &second)
            .expect("fits after the release");
    }

    #[test]
    fn the_count_limit_refuses_and_reserves_nothing() {
        let mut source = Fake::new(1);
        let mut pool = ImagePool::<u64>::new(512 * MIB);
        let held = pool
            .allocate(&mut source, &request(1, 1))
            .expect("the one allocation");
        let mut other = request(1, 1);
        other.memory_type = 5;
        assert_eq!(
            pool.allocate(&mut source, &other)
                .expect_err("the count is at its limit"),
            AllocError::Source(SourceError::Count {
                in_use: 1,
                limit: 1
            })
        );
        assert_eq!(
            pool.reserved(),
            BLOCK_SIZE,
            "the refused block reserved nothing"
        );
        pool.release(&mut source, held);
        pool.allocate(&mut source, &other)
            .expect("a freed allocation frees the count");
    }

    #[test]
    fn a_source_that_refuses_reserves_nothing() {
        let mut source = Fake::new(8);
        source.fail = true;
        let mut pool = ImagePool::<u64>::new(512 * MIB);
        assert!(matches!(
            pool.allocate(&mut source, &request(1, 1)),
            Err(AllocError::Source(SourceError::Device(_)))
        ));
        assert_eq!(pool.reserved(), 0);
        assert_eq!(pool.allocations(), 0);
    }

    #[test]
    fn a_range_is_never_reused_before_its_release() {
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(256 * MIB);
        let mut live: Vec<Placement<u64>> = Vec::new();
        for round in 0..2000_u64 {
            let size = 1 + (round * 7919) % 20_000;
            let alignment = 1 << (round % 9);
            live.push(
                pool.allocate(&mut source, &request(size, alignment))
                    .expect("room"),
            );
            if round % 3 == 2 {
                let released = live.remove(usize::try_from(round % 5).expect("small") % live.len());
                pool.release(&mut source, released);
            }
            let refs: Vec<&Placement<u64>> = live.iter().collect();
            assert!(
                !overlap(&refs),
                "two live placements overlap after round {round}"
            );
        }
        for placement in live {
            pool.release(&mut source, placement);
        }
        assert_eq!(pool.reserved(), 0);
        assert!(source.live.is_empty());
    }

    #[test]
    fn negative_control_freeing_a_range_at_retirement_hands_a_live_range_out_again() {
        // A store that returned the range when the revision was retired (while a frame still read the image)
        // instead of when the image was destroyed: the next image lands on top of the live one, which the
        // overlap check names.
        let mut source = Fake::new(8);
        let mut pool = ImagePool::<u64>::new(256 * MIB);
        let live = pool
            .allocate(&mut source, &request(4096, 256))
            .expect("live");
        let keep_other = pool
            .allocate(&mut source, &request(4096, 256))
            .expect("other");
        let copy = Placement {
            memory: live.memory,
            offset: live.offset,
            size: live.size,
            block: live.block,
        };
        pool.release(&mut source, live); // the early free
        let next = pool
            .allocate(&mut source, &request(4096, 256))
            .expect("next");
        assert!(
            overlap(&[&copy, &next]),
            "the early free let a new image take the live range"
        );
        assert!(!overlap(&[&keep_other, &next]));
        // The honest sequence never overlaps: covered by the previous test.
        pool.release(&mut source, next);
        pool.release(&mut source, keep_other);
    }

    #[test]
    fn align_up_rounds_and_reports_overflow() {
        assert_eq!(align_up(0, 4), Some(0));
        assert_eq!(align_up(1, 4), Some(4));
        assert_eq!(align_up(4, 4), Some(4));
        assert_eq!(align_up(u64::MAX, 4), None);
        assert_eq!(align_up(7, 0), None);
    }
}
