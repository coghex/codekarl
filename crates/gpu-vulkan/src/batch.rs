//! Building a scene frame's instances and batches (milestone 5.5, issue 2a; design §8.1, §13.1).
//!
//! Everything here is plain data and plain Rust, so it is tested without a device: [`build_scene`] asks a
//! [`VulkanSceneSource`] for its quads in painter order, validates each, resolves its key to the revision's descriptor set
//! through a caller-supplied function (the frame ticket in production), writes one [`InstanceRecord`] per drawn quad and
//! groups **adjacent** instances with the same set into one [`Batch`]. Nothing is sorted by texture (a textured alpha
//! scene cannot generally be sorted by texture), and the cost is one validation, one map lookup and one 32-byte write
//! per quad: O(drawn quads), with no scan of the loaded textures. A set belongs to one revision, so equal sets are equal
//! revisions.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

use ash::vk;

use crate::frame::{InstanceRole, VulkanInstance, VulkanSceneSource};
use crate::quad::DestinationRect;

/// The bytes of one instance in the instance buffer: the stride the vertex input reads.
pub(crate) const INSTANCE_STRIDE: usize = size_of::<InstanceRecord>();

/// One instance as the instance-rate vertex input reads it: the destination rectangle in drawable pixels and the
/// visible part of the texture as fractions of it, each `[left, top, right, bottom]`. 32 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct InstanceRecord {
    pub(crate) destination: [f32; 4],
    pub(crate) source: [f32; 4],
}

impl InstanceRecord {
    /// The record's 32 bytes in the order the vertex input reads them (native endianness).
    pub(crate) fn to_bytes(self) -> [u8; INSTANCE_STRIDE] {
        let mut bytes = [0_u8; INSTANCE_STRIDE];
        for (chunk, value) in bytes
            .chunks_exact_mut(4)
            .zip(self.destination.iter().chain(self.source.iter()))
        {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

/// One instanced draw: the instances `first..first + count` of the buffer, all with `set` (the revision's descriptor set,
/// set 1 of the pipeline layout). A null `set` is the checker, whose draw is also the one that samples nearest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Batch {
    pub(crate) set: vk::DescriptorSet,
    pub(crate) first: u32,
    pub(crate) count: u32,
}

impl Batch {
    /// Whether this batch draws the checker.
    pub(crate) fn is_checker(&self) -> bool {
        self.set == vk::DescriptorSet::null()
    }
}

/// What building one frame did (the test hooks observe it; it is not an engine counter).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameCounters {
    /// Instances written, and drawn: the quads of the frame, in painter order.
    pub instances: u64,
    /// Instanced draw calls (one per batch).
    pub batches: u64,
    /// Instances whose key resolved to a shown revision.
    pub resolved: u64,
    /// Instances drawn with the checker (their key has nothing shown yet).
    pub checker: u64,
    /// Instances the presenter rejected as invalid (not finite, outside 0 to 1, or empty); they took no slot.
    pub skipped: u64,
    /// Instances beyond `max_quads`, which are not drawn.
    pub dropped: u64,
    /// Distinct revisions this frame pinned.
    pub pinned: u64,
}

/// What the compatibility quad (the first instance of role [`InstanceRole::Compat`]) is drawn with, decided by the caller
/// that can resolve textures through the frame's ticket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CompatDecision {
    /// Draw it with this set, at this rectangle and source: the current image, or the held one with the rectangle it was
    /// drawn at.
    Draw {
        set: vk::DescriptorSet,
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

/// The instances and batches a frame build writes: one slot per drawn instance, runs of adjacent equal sets sharing a batch.
struct Output<'a> {
    write: &'a mut dyn FnMut(usize, InstanceRecord),
    batches: &'a mut Vec<Batch>,
    capacity: usize,
    written: usize,
    counters: FrameCounters,
}

impl Output<'_> {
    /// Stores one drawn instance (the caller checked the capacity).
    fn push(&mut self, set: vk::DescriptorSet, visible: [f32; 4], destination: DestinationRect) {
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
            },
        );
        let index = u32::try_from(self.written).unwrap_or(u32::MAX);
        match self.batches.last_mut() {
            Some(last) if last.set == set => last.count += 1,
            _ => self.batches.push(Batch {
                set,
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
                set,
                destination,
                source,
            } => {
                if self.written >= self.capacity {
                    self.counters.dropped += 1;
                } else {
                    self.counters.resolved += 1;
                    self.counters.pinned += 1;
                    self.push(set, source, destination);
                }
                false
            }
            CompatDecision::Skip => false,
            CompatDecision::Gate => true,
        }
    }
}

/// Builds one frame: `write(slot, record)` stores each drawn instance (slots `0..capacity`), `batches` receives the draws,
/// and the counters are returned. `resolve` maps a key to its revision's descriptor set, or `None` for the checker;
/// `resolved` is the per-frame cache (cleared here, its allocation reused). The compatibility quad is never drawn with the
/// checker: `compat` decides what it is drawn with. It is asked for the compatibility instance (emitted first, below every
/// scene quad), and once with `None` when the frame has a compatibility entry (`compat_expected`) that placed to nothing,
/// before the first scene quad, so a held image is still drawn while the replacement is pending.
#[expect(
    clippy::too_many_arguments,
    reason = "the source, the size and capacity, the writer, the two resolvers, the cache and the batches the frame needs"
)]
pub(crate) fn build_scene<K: Copy + Eq + Hash, S: BuildHasher>(
    source: &dyn VulkanSceneSource<K>,
    size: (u32, u32),
    capacity: usize,
    write: &mut dyn FnMut(usize, InstanceRecord),
    resolve: &mut dyn FnMut(K) -> Option<vk::DescriptorSet>,
    compat_expected: bool,
    compat: &mut dyn FnMut(Option<&VulkanInstance<K>>) -> CompatDecision,
    resolved: &mut HashMap<K, Option<vk::DescriptorSet>, S>,
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
    };
    let mut gated = false;
    let mut first = true;
    source.place(size.0, size.1, &mut |instance: VulkanInstance<K>| {
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
        let set = *resolved.entry(instance.key).or_insert_with(|| {
            let found = resolve(instance.key);
            output.counters.pinned += u64::from(found.is_some());
            found
        });
        if let Some(set) = set {
            output.counters.resolved += 1;
            output.push(set, instance.source, instance.destination);
        } else {
            output.counters.checker += 1;
            output.push(vk::DescriptorSet::null(), WHOLE, instance.destination);
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
    Built { counters, gated }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<VulkanInstance<u32>>);

    impl VulkanSceneSource<u32> for Fixed {
        fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(VulkanInstance<u32>)) {
            for instance in &self.0 {
                out(*instance);
            }
        }
    }

    fn instance(key: u32, left: i32) -> VulkanInstance<u32> {
        VulkanInstance {
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

    fn handle(texture: usize) -> vk::DescriptorSet {
        use ash::vk::Handle as _;
        vk::DescriptorSet::from_raw(texture as u64)
    }

    struct Run {
        counters: FrameCounters,
        written: Vec<(usize, InstanceRecord)>,
        batches: Vec<Batch>,
        resolutions: Vec<u32>,
    }

    /// Builds `instances` with keys 1 and 2 shown (handles 0x10 and 0x20) and every other key unresolved.
    fn run(instances: Vec<VulkanInstance<u32>>, capacity: usize) -> Run {
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
            set: handle(texture),
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
        assert_eq!(got.counters.batches, 3);
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
        assert_eq!(got.batches[0].set, vk::DescriptorSet::null());
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
        instances: Vec<VulkanInstance<u32>>,
        expected: bool,
        decision: fn(Option<&VulkanInstance<u32>>) -> CompatDecision,
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

    fn compat_instance(key: u32, left: i32) -> VulkanInstance<u32> {
        VulkanInstance {
            role: InstanceRole::Compat,
            ..instance(key, left)
        }
    }

    fn drawn_held(_: Option<&VulkanInstance<u32>>) -> CompatDecision {
        // The held image is another texture, drawn at its own rectangle and source, not the instance's.
        CompatDecision::Draw {
            set: handle(0x30),
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
        assert_eq!(INSTANCE_STRIDE, 32);
        assert_eq!(align_of::<InstanceRecord>(), 4);
        assert_eq!(std::mem::offset_of!(InstanceRecord, source), 16);
    }
}
