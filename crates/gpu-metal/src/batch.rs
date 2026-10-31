//! Building a scene frame's instances and batches (milestone 5, issue 5b-1; design §8.1, §13.1).
//!
//! Everything here is plain data and plain Rust, so it is tested without a device: [`build_scene`] asks a
//! [`MetalSceneSource`] for its quads in painter order, validates each, resolves its key to a texture handle through
//! a caller-supplied function (the frame ticket in production), writes one [`InstanceRecord`] per drawn quad and groups
//! **adjacent** instances with the same texture into one [`Batch`]. Nothing is sorted by texture (a textured alpha scene
//! cannot generally be sorted by texture), and the cost is one validation, one map lookup and one 32-byte write per
//! quad: O(drawn quads), with no scan of the loaded textures.

use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{BuildHasher, Hash};

use crate::frame::{InstanceRole, MetalInstance, MetalSceneSource};
use crate::quad::DestinationRect;

/// The bytes of one instance in the instance buffer (`InstanceData` in `quad.metal`).
pub(crate) const INSTANCE_STRIDE: usize = size_of::<InstanceRecord>();

/// One instance as the instanced vertex function reads it: the destination rectangle in drawable pixels and the visible
/// part of the texture as fractions of it, each `[left, top, right, bottom]`, then the backend-private texture index (the
/// native slot of the image in the bindless table, slot 0 being the checker) and three words of padding. The record is **48
/// bytes** (16 more than the ordinary path's 32): `InstanceData` in `quad.metal` has two `float4` and a `uint4`, so the
/// shader's stride is the `float4` alignment. At 50,000 quads that is 0.8 MB more per instance buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct InstanceRecord {
    pub(crate) destination: [f32; 4],
    pub(crate) source: [f32; 4],
    /// The index into the bindless table (ignored by the ordinary path).
    pub(crate) slot: u32,
    pub(crate) padding: [u32; 3],
}

/// One instanced draw: the instances `first..first + count` of the buffer, all with `texture`. A null `texture` is the
/// checker (`CKPrBatch` in `presenter.h`). In the bindless path a batch is a **run** of instances that share the pipeline,
/// the clip and the filter (all of a scene's): its `texture` is null and each instance selects its own image by its slot.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Batch {
    pub(crate) texture: *mut c_void,
    pub(crate) first: u32,
    pub(crate) count: u32,
}

/// What building one frame did, as the latest value and (in [`SceneStats`](crate::SceneStats)) the peak.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameCounters {
    /// Instances written, and drawn: the quads of the frame, in painter order.
    pub instances: u64,
    /// Instanced draw calls (one per batch).
    pub batches: u64,
    /// Texture binds (one per batch).
    pub texture_binds: u64,
    /// Instances whose key resolved to a shown revision.
    pub resolved: u64,
    /// Instances drawn with the checker (their key has nothing shown yet).
    pub checker: u64,
    /// Instances the presenter rejected as invalid (not finite, outside 0 to 1, or empty); they took no slot.
    pub skipped: u64,
    /// Instances beyond `max_quads`, which are not drawn.
    pub dropped: u64,
    /// Instances whose native slot was outside the pinned table (bindless path): never written, never drawn, counted here.
    pub slot_skipped: u64,
    /// Distinct revisions this frame pinned.
    pub pinned: u64,
    /// Microseconds spent building the frame's draw list (placing, resolving and writing the instances).
    pub build_micros: u64,
    /// Microseconds spent building and encoding the frame (the draw list included).
    pub micros: u64,
}

impl FrameCounters {
    /// Each counter's larger value.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self {
            instances: self.instances.max(other.instances),
            batches: self.batches.max(other.batches),
            texture_binds: self.texture_binds.max(other.texture_binds),
            resolved: self.resolved.max(other.resolved),
            checker: self.checker.max(other.checker),
            skipped: self.skipped.max(other.skipped),
            dropped: self.dropped.max(other.dropped),
            slot_skipped: self.slot_skipped.max(other.slot_skipped),
            pinned: self.pinned.max(other.pinned),
            build_micros: self.build_micros.max(other.build_micros),
            micros: self.micros.max(other.micros),
        }
    }
}

/// A key resolved for a frame: the texture handle (the ordinary path binds it; null in the bindless path, which pins no
/// revision) and its native slot in the pinned table (the bindless path writes it into the instance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) handle: *mut c_void,
    pub(crate) slot: u32,
}

/// How a frame's instances become draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// CK-116's ordinary path: adjacent instances with the same texture share a batch, each batch binds its texture.
    Ordinary,
    /// The bindless path: one run for every instance (texture identity never splits a draw), each instance carrying its slot.
    Bindless {
        /// The entry count of the pinned table: a slot at or beyond it is never written (counted, skipped).
        texture_slots: u32,
        /// The most instances one draw takes. A run is split only where this requires it, in order, so painter order holds.
        /// Named limits are the instance buffer's capacity (`max_quads`, which `capacity` already bounds) and, in tests, an
        /// injected smaller value; Metal documents no smaller `instanceCount` limit than `u32` for this device family.
        chunk: u32,
        /// Tests only: the `n`-th drawn instance's slot is replaced by one past the table (a forced out-of-range slot).
        force_out_of_range_at: Option<usize>,
    },
}

/// What the compatibility quad (the first instance of role [`InstanceRole::Compat`]) is drawn with, decided by the caller
/// that can resolve textures through the frame's ticket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CompatDecision {
    /// Draw it with this texture, at this rectangle and source: the current image, or the held one with the rectangle it was
    /// drawn at.
    Draw {
        texture: *mut c_void,
        destination: DestinationRect,
        source: [f32; 4],
    },
    /// [`Draw`](Self::Draw) in the bindless path: the image is named by its slot in the pinned table, not by a handle.
    DrawSlot {
        slot: u32,
        destination: DestinationRect,
        source: [f32; 4],
    },
    /// Draw nothing for it: the current image resolved and is placed to nothing, or there is no image and none was drawn.
    Skip,
    /// Neither the current image nor a held one resolves: the frame is not drawn (the initial gate).
    Gate,
}

/// The checker's source rectangle: the whole texture.
const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A drawable pixel coordinate as the shader reads it.
#[expect(
    clippy::cast_precision_loss,
    reason = "drawable pixel coordinates are far below 2^24, where i32 to f32 is exact"
)]
fn pixel(value: i32) -> f32 {
    value as f32
}

/// What building one frame came to.
pub(crate) struct Built {
    pub(crate) counters: FrameCounters,
    /// The initial gate: the compatibility quad had no image to draw and no earlier one was held, so the frame must not be
    /// drawn (what was written is abandoned).
    pub(crate) gated: bool,
}

/// The instances and batches a frame build writes: one slot per drawn instance, runs of adjacent equal textures sharing a
/// batch.
struct Output<'a> {
    write: &'a mut dyn FnMut(usize, InstanceRecord),
    batches: &'a mut Vec<Batch>,
    capacity: usize,
    written: usize,
    counters: FrameCounters,
    layout: Layout,
    /// The drawn instances so far (the forced out-of-range hook counts them).
    drawn: usize,
}

impl Output<'_> {
    /// Stores one drawn instance (the caller checked the capacity). In the bindless path the slot is bounds-checked first:
    /// one at or beyond the table's entry count is never written (it takes no capacity), is counted in `slot_skipped`, and
    /// is a debug assertion unless a test forced it.
    fn push(
        &mut self,
        handle: *mut c_void,
        slot: u32,
        visible: [f32; 4],
        destination: DestinationRect,
    ) {
        let mut slot = slot;
        if let Layout::Bindless {
            texture_slots,
            force_out_of_range_at,
            ..
        } = self.layout
        {
            let forced = force_out_of_range_at == Some(self.drawn);
            self.drawn += 1;
            if forced {
                slot = texture_slots;
            }
            if slot >= texture_slots {
                debug_assert!(
                    forced,
                    "the slot {slot} is outside the table of {texture_slots}"
                );
                self.counters.slot_skipped += 1;
                return;
            }
        }
        (self.write)(
            self.written,
            InstanceRecord {
                destination: [
                    pixel(destination.left),
                    pixel(destination.top),
                    pixel(destination.right),
                    pixel(destination.bottom),
                ],
                source: visible,
                slot,
                padding: [0; 3],
            },
        );
        let index = u32::try_from(self.written).unwrap_or(u32::MAX);
        match (self.layout, self.batches.last_mut()) {
            // The bindless run grows until the named limit, then a new run starts right after it.
            (Layout::Bindless { chunk, .. }, Some(last)) if last.count < chunk.max(1) => {
                last.count += 1;
            }
            (Layout::Ordinary, Some(last)) if last.texture == handle => last.count += 1,
            _ => self.batches.push(Batch {
                texture: if matches!(self.layout, Layout::Ordinary) {
                    handle
                } else {
                    std::ptr::null_mut()
                },
                first: index,
                count: 1,
            }),
        }
        self.written += 1;
    }

    /// Applies the compatibility decision. `true` when the frame is gated. A drawn compatibility image counts as resolved and
    /// pinned; it takes a slot like any instance.
    fn compat(&mut self, decision: CompatDecision) -> bool {
        match decision {
            CompatDecision::Draw {
                texture,
                destination,
                source,
            } => {
                if self.written >= self.capacity {
                    self.counters.dropped += 1;
                } else {
                    self.counters.resolved += 1;
                    self.counters.pinned += 1;
                    self.push(texture, 0, source, destination);
                }
                false
            }
            CompatDecision::DrawSlot {
                slot,
                destination,
                source,
            } => {
                // The bindless path pins no revision: the table epoch alone keeps the image alive.
                if self.written >= self.capacity {
                    self.counters.dropped += 1;
                } else {
                    self.counters.resolved += 1;
                    self.push(std::ptr::null_mut(), slot, source, destination);
                }
                false
            }
            CompatDecision::Skip => false,
            CompatDecision::Gate => true,
        }
    }
}

/// Builds one frame: `write(slot, record)` stores each drawn instance (slots `0..capacity`), `batches` receives the draws,
/// and the counters are returned. `resolve` maps a key to its texture handle, or `None` for the checker; `resolved` is the
/// per-frame cache (cleared here, its allocation reused). The compatibility quad is never drawn with the checker: `compat`
/// decides what it is drawn with. It is asked for the compatibility instance (emitted first, below every scene quad), and
/// once with `None` when the frame has a compatibility entry (`compat_expected`) that placed to nothing, before the first
/// scene quad, so a held image is still drawn while the replacement is pending.
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "the source, the size and capacity, the writer, the two resolvers, the cache and the batches the frame needs"
)]
pub(crate) fn build_scene<K: Copy + Eq + Hash>(
    source: &dyn MetalSceneSource<K>,
    size: (u32, u32),
    capacity: usize,
    write: &mut dyn FnMut(usize, InstanceRecord),
    resolve: &mut dyn FnMut(K) -> Option<*mut c_void>,
    compat_expected: bool,
    compat: &mut dyn FnMut(Option<&MetalInstance<K>>) -> CompatDecision,
    resolved: &mut HashMap<K, Option<*mut c_void>>,
    batches: &mut Vec<Batch>,
) -> Built {
    let mut cache = HashMap::new();
    let built = build_scene_in(
        Layout::Ordinary,
        source,
        size,
        capacity,
        write,
        &mut |key| resolve(key).map(|handle| Resolved { handle, slot: 0 }),
        compat_expected,
        compat,
        &mut cache,
        batches,
    );
    resolved.clear();
    resolved.extend(
        cache
            .into_iter()
            .map(|(key, found)| (key, found.map(|found| found.handle))),
    );
    built
}

/// [`build_scene`] in either [`Layout`]: `resolve` maps a key to its [`Resolved`] texture (handle and native slot), or
/// `None` for the checker (slot 0 in the bindless path).
#[expect(
    clippy::too_many_arguments,
    reason = "the layout, the source, the size and capacity, the writer, the two resolvers, the cache and the batches the frame needs"
)]
pub(crate) fn build_scene_in<K: Copy + Eq + Hash, S: BuildHasher>(
    layout: Layout,
    source: &dyn MetalSceneSource<K>,
    size: (u32, u32),
    capacity: usize,
    write: &mut dyn FnMut(usize, InstanceRecord),
    resolve: &mut dyn FnMut(K) -> Option<Resolved>,
    compat_expected: bool,
    compat: &mut dyn FnMut(Option<&MetalInstance<K>>) -> CompatDecision,
    resolved: &mut HashMap<K, Option<Resolved>, S>,
    batches: &mut Vec<Batch>,
) -> Built {
    resolved.clear();
    batches.clear();
    let mut output = Output {
        write,
        batches,
        capacity,
        written: 0,
        counters: FrameCounters::default(),
        layout,
        drawn: 0,
    };
    let mut gated = false;
    let mut first = true;
    source.place(size.0, size.1, &mut |instance: MetalInstance<K>| {
        if gated {
            return;
        }
        // The compatibility entry placed to nothing: the first instance is a scene quad (or there is none, below).
        if std::mem::take(&mut first) && compat_expected && instance.role != InstanceRole::Compat {
            gated = output.compat(compat(None));
            if gated {
                return;
            }
        }
        if !instance.is_valid() {
            output.counters.skipped += 1;
            return;
        }
        if instance.role == InstanceRole::Compat {
            gated = output.compat(compat(Some(&instance)));
            return;
        }
        if output.written >= output.capacity {
            output.counters.dropped += 1;
            return;
        }
        let texture = *resolved.entry(instance.key).or_insert_with(|| {
            let found = resolve(instance.key);
            // Only the ordinary path pins a revision per distinct image; the bindless path pins the table epoch alone.
            output.counters.pinned +=
                u64::from(found.is_some() && matches!(layout, Layout::Ordinary));
            found
        });
        if let Some(texture) = texture {
            output.counters.resolved += 1;
            output.push(
                texture.handle,
                texture.slot,
                instance.source,
                instance.destination,
            );
        } else {
            output.counters.checker += 1;
            output.push(std::ptr::null_mut(), 0, WHOLE, instance.destination);
        }
    });
    if first && compat_expected && !gated {
        // Nothing was placed at all, the compatibility entry included.
        gated = output.compat(compat(None));
    }
    let Output {
        mut counters,
        written,
        batches,
        ..
    } = output;
    counters.instances = written as u64;
    counters.batches = batches.len() as u64;
    // The ordinary path binds a texture per batch; the bindless path binds the table once for the frame.
    counters.texture_binds = if matches!(layout, Layout::Ordinary) {
        counters.batches
    } else {
        u64::from(!batches.is_empty())
    };
    Built { counters, gated }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quad::DestinationRect;

    struct Fixed(Vec<MetalInstance<u32>>);

    impl MetalSceneSource<u32> for Fixed {
        fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(MetalInstance<u32>)) {
            for instance in &self.0 {
                out(*instance);
            }
        }
    }

    fn instance(key: u32, left: i32) -> MetalInstance<u32> {
        MetalInstance {
            key,
            destination: DestinationRect {
                left,
                top: 0,
                right: left + 4,
                bottom: 4,
            },
            source: [0.25, 0.25, 0.75, 0.75],
            role: InstanceRole::Scene,
        }
    }

    fn bits(values: [f32; 4]) -> [u32; 4] {
        values.map(f32::to_bits)
    }

    fn handle(texture: usize) -> *mut c_void {
        texture as *mut c_void
    }

    struct Run {
        counters: FrameCounters,
        written: Vec<(usize, InstanceRecord)>,
        batches: Vec<Batch>,
        resolutions: Vec<u32>,
    }

    /// Builds `instances` with keys 1 and 2 shown (handles 0x10 and 0x20) and every other key unresolved.
    fn run(instances: Vec<MetalInstance<u32>>, capacity: usize) -> Run {
        let mut written = Vec::new();
        let mut resolutions = Vec::new();
        let mut batches = Vec::new();
        let mut resolved = HashMap::new();
        let counters = build_scene(
            &Fixed(instances),
            (100, 100),
            capacity,
            &mut |index, record| written.push((index, record)),
            &mut |key| {
                resolutions.push(key);
                match key {
                    1 => Some(handle(0x10)),
                    2 => Some(handle(0x20)),
                    _ => None,
                }
            },
            false,
            &mut |_| CompatDecision::Gate,
            &mut resolved,
            &mut batches,
        )
        .counters;
        Run {
            counters,
            written,
            batches,
            resolutions,
        }
    }

    fn batch(texture: usize, first: u32, count: u32) -> Batch {
        Batch {
            texture: handle(texture),
            first,
            count,
        }
    }

    #[test]
    fn instances_are_written_in_painter_order_and_adjacent_equal_textures_share_a_batch() {
        // Negative control: sorting by texture, or batching any repeat rather than an adjacent one, gives
        // `[A A A, B]` for the four instances A A B A.
        let got = run(
            vec![
                instance(1, 0),
                instance(1, 4),
                instance(2, 8),
                instance(1, 12),
            ],
            16,
        );
        assert_eq!(
            got.batches,
            vec![batch(0x10, 0, 2), batch(0x20, 2, 1), batch(0x10, 3, 1)]
        );
        let lefts: Vec<f32> = got.written.iter().map(|(_, r)| r.destination[0]).collect();
        assert_eq!(lefts, [0.0, 4.0, 8.0, 12.0], "painter order");
        let slots: Vec<usize> = got.written.iter().map(|(slot, _)| *slot).collect();
        assert_eq!(slots, [0, 1, 2, 3]);
        assert_eq!(got.counters.instances, 4);
        assert_eq!((got.counters.batches, got.counters.texture_binds), (3, 3));
        assert_eq!((got.counters.resolved, got.counters.checker), (4, 0));
    }

    #[test]
    fn a_thousand_instances_of_one_texture_are_one_draw_and_alternating_ones_are_one_each() {
        let same: Vec<_> = (0..1000).map(|i| instance(1, i)).collect();
        assert_eq!(run(same, 4096).batches, vec![batch(0x10, 0, 1000)]);
        let alternating: Vec<_> = (0..1000)
            .map(|i| instance(if i % 2 == 0 { 1 } else { 2 }, i))
            .collect();
        let got = run(alternating, 4096);
        assert_eq!(got.batches.len(), 1000);
        assert_eq!(got.counters.batches, 1000);
        assert!(got.batches.iter().all(|b| b.count == 1));
    }

    #[test]
    fn a_key_is_resolved_once_per_frame_however_often_it_is_drawn() {
        let got = run(
            vec![
                instance(1, 0),
                instance(2, 4),
                instance(1, 8),
                instance(1, 12),
                instance(7, 16),
                instance(7, 20),
            ],
            16,
        );
        assert_eq!(
            got.resolutions,
            vec![1, 2, 7],
            "one resolution per distinct key, in first-use order"
        );
        assert_eq!(got.counters.pinned, 2, "the unresolved key pins nothing");
    }

    #[test]
    fn an_unresolved_key_draws_the_checker_over_its_whole_destination() {
        let got = run(vec![instance(7, 0), instance(7, 4), instance(1, 8)], 16);
        assert_eq!(got.batches[0].texture, std::ptr::null_mut());
        assert_eq!((got.batches[0].first, got.batches[0].count), (0, 2));
        assert_eq!(
            bits(got.written[0].1.source),
            bits(WHOLE),
            "the checker ignores the instance's source"
        );
        assert_eq!(
            bits(got.written[2].1.source),
            bits([0.25, 0.25, 0.75, 0.75])
        );
        assert_eq!((got.counters.checker, got.counters.resolved), (2, 1));
    }

    #[test]
    fn a_rejected_instance_takes_no_slot_and_does_not_break_adjacency() {
        let mut bad = instance(1, 4);
        bad.source = [0.0, 0.0, f32::NAN, 1.0];
        let mut empty = instance(1, 6);
        empty.destination.right = empty.destination.left;
        let got = run(vec![instance(1, 0), bad, empty, instance(1, 8)], 2);
        assert_eq!(got.counters.skipped, 2);
        assert_eq!(
            got.counters.instances, 2,
            "the two valid ones fit a capacity of two"
        );
        assert_eq!(
            got.counters.dropped, 0,
            "rejected instances consumed no capacity"
        );
        assert_eq!(got.batches, vec![batch(0x10, 0, 2)]);
        assert_eq!(got.written.len(), 2);
    }

    #[test]
    fn instances_beyond_the_capacity_are_dropped_counted_and_never_written_or_resolved() {
        let many: Vec<_> = (0..10_u32)
            .map(|i| instance(3 + i, i32::try_from(i).expect("small")))
            .collect();
        let got = run(many, 4);
        assert_eq!(got.counters.instances, 4);
        assert_eq!(got.counters.dropped, 6);
        assert_eq!(got.written.len(), 4);
        assert!(
            got.written.iter().all(|(slot, _)| *slot < 4),
            "nothing past the buffer"
        );
        assert_eq!(
            got.resolutions,
            vec![3, 4, 5, 6],
            "a dropped instance is not resolved"
        );
    }

    /// What building with a compatibility decision gave.
    struct Compat {
        built: Built,
        written: Vec<(usize, InstanceRecord)>,
        batches: Vec<Batch>,
        resolutions: Vec<u32>,
        asked: Vec<Option<u32>>,
    }

    /// Builds `instances` with `decision` answering for the compatibility instance (keys 1 and 2 shown, as in `run`).
    fn run_compat(
        instances: Vec<MetalInstance<u32>>,
        expected: bool,
        decision: fn(Option<&MetalInstance<u32>>) -> CompatDecision,
    ) -> Compat {
        let mut written = Vec::new();
        let mut resolutions = Vec::new();
        let mut asked = Vec::new();
        let mut batches = Vec::new();
        let mut resolved = HashMap::new();
        let built = build_scene(
            &Fixed(instances),
            (100, 100),
            16,
            &mut |index, record| written.push((index, record)),
            &mut |key| {
                resolutions.push(key);
                match key {
                    1 => Some(handle(0x10)),
                    2 => Some(handle(0x20)),
                    _ => None,
                }
            },
            expected,
            &mut |instance| {
                asked.push(instance.map(|instance| instance.key));
                decision(instance)
            },
            &mut resolved,
            &mut batches,
        );
        Compat {
            built,
            written,
            batches,
            resolutions,
            asked,
        }
    }

    fn compat_instance(key: u32, left: i32) -> MetalInstance<u32> {
        MetalInstance {
            role: InstanceRole::Compat,
            ..instance(key, left)
        }
    }

    fn drawn_held(_: Option<&MetalInstance<u32>>) -> CompatDecision {
        // The held image is another texture, drawn at its own rectangle and source, not the instance's.
        CompatDecision::Draw {
            texture: handle(0x30),
            destination: DestinationRect {
                left: 50,
                top: 51,
                right: 60,
                bottom: 61,
            },
            source: [0.0, 0.0, 1.0, 1.0],
        }
    }

    #[test]
    fn a_compat_instance_is_drawn_as_the_decision_says_and_never_resolved_or_checkered() {
        // Negative control: resolving the compat key like a scene key would call `resolve(9)` and draw the checker.
        let got = run_compat(
            vec![compat_instance(9, 0), instance(1, 4), instance(7, 8)],
            true,
            drawn_held,
        );
        assert!(!got.built.gated);
        assert_eq!(
            got.asked,
            [Some(9)],
            "asked once, for the compat instance only"
        );
        assert_eq!(
            got.resolutions,
            vec![1, 7],
            "the compat key is never resolved as a scene key"
        );
        assert_eq!(
            got.batches,
            vec![batch(0x30, 0, 1), batch(0x10, 1, 1), batch(0, 2, 1)],
            "the compat image first, below the scene quads"
        );
        assert_eq!(
            bits(got.written[0].1.destination),
            bits([50.0, 51.0, 60.0, 61.0]),
            "the decision's own rectangle"
        );
        assert_eq!(bits(got.written[0].1.source), bits([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(
            (got.built.counters.resolved, got.built.counters.checker),
            (2, 1),
            "the compat draw counts as resolved; only the unresolved scene key is the checker"
        );
        assert_eq!(got.built.counters.pinned, 2, "the compat pin and key 1's");
    }

    #[test]
    fn a_gated_compat_instance_stops_the_frame_before_anything_after_it_is_written_or_resolved() {
        // Negative control: continuing past the gate writes the scene quads of a frame that will be discarded.
        let got = run_compat(
            vec![compat_instance(9, 0), instance(1, 4), instance(2, 8)],
            true,
            |_| CompatDecision::Gate,
        );
        assert!(got.built.gated);
        assert!(got.written.is_empty() && got.batches.is_empty());
        assert!(got.resolutions.is_empty(), "no later key is resolved");
    }

    #[test]
    fn a_frame_without_a_compat_instance_never_asks_for_a_decision() {
        let got = run_compat(vec![instance(1, 0), instance(7, 4)], false, |_| {
            CompatDecision::Gate
        });
        assert!(got.asked.is_empty() && !got.built.gated);
        assert_eq!(got.batches.len(), 2);
    }

    #[test]
    fn a_compat_entry_that_placed_to_nothing_is_asked_once_before_the_first_scene_quad() {
        // The held image is drawn first, below the scene quads, at the rectangle the decision names. Negative control: asking
        // only for emitted compat instances draws nothing for a clipped pending replacement.
        let got = run_compat(vec![instance(1, 4), instance(7, 8)], true, drawn_held);
        assert_eq!(got.asked, [None], "asked once, without an instance");
        assert_eq!(
            got.batches,
            vec![batch(0x30, 0, 1), batch(0x10, 1, 1), batch(0, 2, 1)]
        );
        assert_eq!(
            bits(got.written[0].1.destination),
            bits([50.0, 51.0, 60.0, 61.0])
        );
        assert_eq!(
            (got.built.counters.resolved, got.built.counters.pinned),
            (2, 2)
        );
        assert!(!got.built.gated);
    }

    #[test]
    fn a_compat_entry_with_nothing_placed_at_all_is_still_asked() {
        let got = run_compat(Vec::new(), true, drawn_held);
        assert_eq!(got.asked, [None]);
        assert_eq!(got.batches, vec![batch(0x30, 0, 1)]);
        assert_eq!(got.built.counters.instances, 1);
    }

    #[test]
    fn a_skipped_compat_entry_draws_nothing_and_a_gated_one_gates() {
        let skipped = run_compat(vec![instance(1, 4)], true, |_| CompatDecision::Skip);
        assert_eq!(skipped.asked, [None]);
        assert_eq!(skipped.batches, vec![batch(0x10, 0, 1)]);
        assert!(!skipped.built.gated);
        let gated = run_compat(vec![instance(1, 4)], true, |_| CompatDecision::Gate);
        assert!(gated.built.gated && gated.resolutions.is_empty());
        // A frame that expects no compatibility entry never asks.
        let none = run_compat(vec![instance(1, 4)], false, |_| CompatDecision::Gate);
        assert!(none.asked.is_empty() && !none.built.gated);
    }

    #[test]
    fn a_scene_of_nothing_builds_nothing() {
        let got = run(Vec::new(), 4);
        assert_eq!(got.counters, FrameCounters::default());
        assert!(got.batches.is_empty() && got.written.is_empty());
    }

    #[test]
    fn the_record_is_the_stride_the_shader_reads() {
        assert_eq!(INSTANCE_STRIDE, 48);
        assert_eq!(align_of::<InstanceRecord>(), 4);
        assert_eq!(std::mem::offset_of!(InstanceRecord, slot), 32);
    }
}
