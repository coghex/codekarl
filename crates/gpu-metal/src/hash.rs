//! A fast hasher for the maps the frame build touches, and nothing else (CK-148).
//!
//! A frame of the milestone 5 scale scene looks up about 229,000 keys of 20 bytes (a `TextureKey`: a `u64` registry, a `u32` slot and a
//! `u64` generation), and the standard library's default hasher (`SipHash-1-3`) costs more than the lookup itself. [`FrameHasher`] is the
//! `FxHash` algorithm: one rotate, one xor and one multiply per word.
//!
//! **Why it is safe here.** `SipHash` is keyed to stop an attacker who chooses the keys from making a table degenerate. Every key in the
//! maps below is made inside the process: registry keys (a registry identity, a slot and a generation the registry hands out) and revision
//! numbers the ledger counts. Nothing outside the process chooses them, so there is no collision flooding to defend against.
//!
//! It is used only for [`FrameMap`] and [`FrameSet`], the maps of the frame build: the scene's per-frame resolution cache, the ledger's
//! per-ticket `resolved` and `pinned`, its `keys` and each key's `revisions`. No other map changes.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// The multiplier of `FxHash` (a 64-bit odd constant derived from the golden ratio).
const SEED: u64 = 0x517c_c1b7_2722_0a95;

/// The `FxHash` hasher: `h = (h.rotate_left(5) ^ word).wrapping_mul(SEED)` for each word written.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FrameHasher {
    hash: u64,
}

impl FrameHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FrameHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut word = [0_u8; 8];
            word.copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0_u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// A map hashed with [`FrameHasher`].
pub(crate) type FrameMap<K, V> = HashMap<K, V, BuildHasherDefault<FrameHasher>>;
/// A set hashed with [`FrameHasher`].
pub(crate) type FrameSet<K> = HashSet<K, BuildHasherDefault<FrameHasher>>;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};

    use super::FrameHasher;

    /// A key shaped like `TextureKey`: a `u64` registry, a `u32` slot and a `u64` generation.
    #[derive(Hash, Clone, Copy)]
    struct Key {
        registry: u64,
        slot: u32,
        generation: u64,
    }

    /// The same shape, hashed by a hasher that ignores `write_u32` (the negative control).
    #[derive(Default)]
    struct IgnoresU32(FrameHasher);

    impl Hasher for IgnoresU32 {
        fn write(&mut self, bytes: &[u8]) {
            self.0.write(bytes);
        }
        fn write_u32(&mut self, _value: u32) {}
        fn write_u64(&mut self, value: u64) {
            self.0.write_u64(value);
        }
        fn finish(&self) -> u64 {
            self.0.finish()
        }
    }

    fn hashes<H: Hasher + Default>(keys: &[Key]) -> Vec<u64> {
        let build = BuildHasherDefault::<H>::default();
        keys.iter().map(|key| build.hash_one(key)).collect()
    }

    /// Whether `hashes` are all distinct, no low-16-bit value occurs more than 8 times and every top-7-bit value occurs.
    fn spreads(hashes: &[u64]) -> bool {
        let mut distinct = hashes.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        let mut low: HashMap<u64, u32> = HashMap::new();
        let mut top = [false; 128];
        for hash in hashes {
            *low.entry(hash & 0xFFFF).or_default() += 1;
            top[usize::try_from(hash >> 57).expect("seven bits")] = true;
        }
        distinct.len() == hashes.len()
            && low.values().all(|count| *count <= 8)
            && top.iter().all(|seen| *seen)
    }

    fn sequential_slots() -> Vec<Key> {
        (0..32_768)
            .map(|slot| Key {
                registry: 7,
                slot,
                generation: 1,
            })
            .collect()
    }

    fn sequential_generations() -> Vec<Key> {
        (0..32_768)
            .map(|generation| Key {
                registry: 7,
                slot: 5,
                generation,
            })
            .collect()
    }

    #[test]
    fn the_frame_hasher_spreads_every_field_of_a_key() {
        assert!(
            spreads(&hashes::<FrameHasher>(&sequential_slots())),
            "sequential slots"
        );
        assert!(
            spreads(&hashes::<FrameHasher>(&sequential_generations())),
            "sequential generations"
        );
    }

    #[test]
    fn a_hasher_that_ignores_the_slot_fails_the_spread_check() {
        // Negative control: one hash for every key, so the first check (all distinct) fails.
        let ignoring = hashes::<IgnoresU32>(&sequential_slots());
        assert!(ignoring.iter().all(|hash| *hash == ignoring[0]));
        assert!(!spreads(&ignoring));
    }

    #[test]
    fn bytes_are_taken_in_eight_byte_chunks_and_a_remainder() {
        let mut whole = FrameHasher::default();
        whole.write(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let mut parts = FrameHasher::default();
        parts.write_u64(u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
        parts.write_u64(9);
        assert_eq!(whole.finish(), parts.finish());
    }
}
