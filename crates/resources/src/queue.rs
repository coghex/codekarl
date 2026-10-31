//! A FIFO of texture keys with O(1) push, pop and removal by slot, and no stale entries.
//!
//! Every resource is in a queue at most once, so the queue never holds more entries than there are resources. It is a
//! doubly linked list threaded through a vector indexed by slot: removing a resource from the middle (its request was
//! replaced, its revision superseded, it retired) unlinks one node and touches its two neighbours, whatever the length
//! of the queue, so the service's bounded steps never scan an unrelated backlog under its lock. A node keeps the
//! generation of the incarnation it names; the registry's identity and the slot make up the rest of the key.

use crate::registry::TextureKey;
use crate::token::RegistryId;

/// "No slot": the end of the list.
const NONE: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Link {
    prev: u32,
    next: u32,
    /// The generation of the queued incarnation.
    generation: u64,
    linked: bool,
}

const UNLINKED: Link = Link {
    prev: NONE,
    next: NONE,
    generation: 0,
    linked: false,
};

/// The queue: oldest first.
pub(crate) struct IndexedQueue {
    registry: RegistryId,
    links: Vec<Link>,
    head: u32,
    tail: u32,
    len: usize,
    /// Nodes read or written so far (tests only: a deterministic measure of the work an operation does).
    #[cfg(test)]
    touches: usize,
}

impl IndexedQueue {
    pub(crate) fn new(registry: RegistryId) -> Self {
        Self {
            registry,
            links: Vec::new(),
            head: NONE,
            tail: NONE,
            len: 0,
            #[cfg(test)]
            touches: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the resource in `slot` is queued.
    pub(crate) fn contains(&self, slot: u32) -> bool {
        usize::try_from(slot)
            .ok()
            .and_then(|index| self.links.get(index))
            .is_some_and(|link| link.linked)
    }

    #[cfg(test)]
    fn touch(&mut self, nodes: usize) {
        self.touches += nodes;
    }

    /// The nodes this queue has read or written (tests only).
    #[cfg(test)]
    pub(crate) fn touches(&self) -> usize {
        self.touches
    }

    /// Puts `key` at the back. A resource already queued moves to the back: the queue holds each resource once.
    pub(crate) fn push_back(&mut self, key: TextureKey) {
        let slot = key.slot();
        self.remove(slot);
        let index = usize::try_from(slot).unwrap_or(usize::MAX);
        if self.links.len() <= index {
            self.links.resize(index + 1, UNLINKED);
        }
        let previous_tail = self.tail;
        self.links[index] = Link {
            prev: previous_tail,
            next: NONE,
            generation: key.generation(),
            linked: true,
        };
        if previous_tail == NONE {
            self.head = slot;
        } else {
            self.links[usize::try_from(previous_tail).unwrap_or(usize::MAX)].next = slot;
        }
        self.tail = slot;
        self.len += 1;
        #[cfg(test)]
        self.touch(2);
    }

    /// The oldest entry, left in place.
    pub(crate) fn front(&self) -> Option<TextureKey> {
        if self.head == NONE {
            None
        } else {
            self.key_of(self.head)
        }
    }

    /// Takes the oldest entry out.
    pub(crate) fn pop_front(&mut self) -> Option<TextureKey> {
        if self.head == NONE {
            return None;
        }
        let slot = self.head;
        let key = self.key_of(slot);
        self.remove(slot);
        key
    }

    /// Takes the resource in `slot` out of the queue; whether it was there. Touches that node and its two neighbours.
    pub(crate) fn remove(&mut self, slot: u32) -> bool {
        let index = usize::try_from(slot).unwrap_or(usize::MAX);
        let Some(&Link {
            prev, next, linked, ..
        }) = self.links.get(index)
        else {
            return false;
        };
        if !linked {
            return false;
        }
        if prev == NONE {
            self.head = next;
        } else {
            self.links[usize::try_from(prev).unwrap_or(usize::MAX)].next = next;
        }
        if next == NONE {
            self.tail = prev;
        } else {
            self.links[usize::try_from(next).unwrap_or(usize::MAX)].prev = prev;
        }
        self.links[index] = UNLINKED;
        self.len -= 1;
        #[cfg(test)]
        self.touch(3);
        true
    }

    /// Empties the queue: O(its length).
    pub(crate) fn clear(&mut self) {
        while self.pop_front().is_some() {}
    }

    fn key_of(&self, slot: u32) -> Option<TextureKey> {
        let link = self.links.get(usize::try_from(slot).ok()?)?;
        link.linked
            .then(|| TextureKey::new(self.registry, slot, link.generation))
    }

    /// The queued keys, oldest first (tests only).
    #[cfg(test)]
    pub(crate) fn keys(&self) -> Vec<TextureKey> {
        let mut keys = Vec::with_capacity(self.len);
        let mut at = self.head;
        while at != NONE {
            keys.extend(self.key_of(at));
            at = self.links[usize::try_from(at).unwrap_or(usize::MAX)].next;
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(registry: RegistryId, slot: u32, generation: u64) -> TextureKey {
        TextureKey::new(registry, slot, generation)
    }

    fn registry() -> RegistryId {
        RegistryId::allocate().expect("an identity")
    }

    #[test]
    fn it_is_a_fifo_that_holds_each_resource_once() {
        let id = registry();
        let mut queue = IndexedQueue::new(id);
        assert!(queue.is_empty() && queue.pop_front().is_none());
        for slot in [3, 1, 2] {
            queue.push_back(key(id, slot, 7));
        }
        assert_eq!(queue.len(), 3);
        // A resource pushed again moves to the back, with its new generation.
        queue.push_back(key(id, 3, 8));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.keys(), [key(id, 1, 7), key(id, 2, 7), key(id, 3, 8)]);
        assert_eq!(queue.pop_front(), Some(key(id, 1, 7)));
        assert!(!queue.contains(1) && queue.contains(2));
    }

    #[test]
    fn removal_from_the_middle_the_ends_and_an_absent_slot() {
        let id = registry();
        let mut queue = IndexedQueue::new(id);
        for slot in 0..5 {
            queue.push_back(key(id, slot, 1));
        }
        assert!(queue.remove(2));
        assert!(!queue.remove(2), "already out");
        assert!(!queue.remove(99), "never queued");
        assert!(queue.remove(0), "the head");
        assert!(queue.remove(4), "the tail");
        assert_eq!(queue.keys(), [key(id, 1, 1), key(id, 3, 1)]);
        queue.push_back(key(id, 0, 2));
        assert_eq!(queue.keys(), [key(id, 1, 1), key(id, 3, 1), key(id, 0, 2)]);
        queue.clear();
        assert!(queue.is_empty() && queue.keys().is_empty());
    }

    /// The work of one removal does not depend on how many other entries are queued.
    #[test]
    fn a_removal_touches_a_constant_number_of_nodes_whatever_the_backlog() {
        let id = registry();
        let mut costs = Vec::new();
        for backlog in [10_u32, 100, 10_000] {
            let mut queue = IndexedQueue::new(id);
            for slot in 0..backlog {
                queue.push_back(key(id, slot, 1));
            }
            let before = queue.touches();
            assert!(queue.remove(backlog / 2));
            let removal = queue.touches() - before;
            let before = queue.touches();
            assert!(queue.pop_front().is_some());
            costs.push((removal, queue.touches() - before));
        }
        assert!(costs.windows(2).all(|pair| pair[0] == pair[1]), "{costs:?}");
        assert!(costs[0].0 <= 3 && costs[0].1 <= 3, "{costs:?}");
    }
}
