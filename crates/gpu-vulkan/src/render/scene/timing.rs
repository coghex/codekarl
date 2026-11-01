//! The scene presenter's frame timing on the render thread (milestone 5.5, issue 6, part B): the six CPU marks of a frame step in the
//! Vulkan order, the two timestamp queries read after the frame's recorded proof, and the bounded polling of
//! `VK_GOOGLE_display_timing`'s feedback. The pure rules (intervals, tick conversion, the pending protocol) are in
//! [`crate::metrics`]; this file takes the clock and calls the device.

use ash::vk;

use crate::batch::FrameCounters;
use crate::gpu::Gpu;
use crate::metrics::{FrameMarks, MetricsShared, gpu_ns};

/// The records one poll reads at most from the presentation engine, per batch, and the batches per poll.
const BATCH: usize = 8;
const BATCHES: usize = 4;
/// How long an idle render thread waits between polls while a present's feedback is pending.
pub(crate) const PRESENTATION_POLL_MS: u64 = 50;
/// The present fences remembered, to say when a present's fence was seen signalled.
const FENCE_MEMORY: usize = 8;

/// One poll of the presentation engine's feedback: at most [`BATCHES`] calls of `next`, each into a buffer of [`BATCH`] records and
/// each answering how many it wrote and whether more remain (`VK_INCOMPLETE`); every non-empty batch goes to `deliver`. It stops
/// at the first call that says nothing remains, or that wrote nothing, or that failed, or after the fourth batch: a backlog is not
/// drained in one poll (the rest comes at the next). Returns the number of calls made.
pub(crate) fn drain_feedback(
    mut next: impl FnMut(&mut [vk::PastPresentationTimingGOOGLE]) -> Result<(usize, bool), vk::Result>,
    mut deliver: impl FnMut(&[vk::PastPresentationTimingGOOGLE]),
) -> usize {
    let mut records = [vk::PastPresentationTimingGOOGLE::default(); BATCH];
    let mut calls = 0;
    for _ in 0..BATCHES {
        calls += 1;
        let Ok((written, more)) = next(&mut records) else {
            break;
        };
        if written != 0 {
            deliver(&records[..written]);
        }
        if !more || written == 0 {
            break;
        }
    }
    calls
}

/// A submitted frame whose GPU time is to be read after its recorded proof.
#[derive(Clone, Copy)]
struct GpuWait {
    /// The frame's submission number (what the texture ledger's proof record answers for).
    number: u64,
    /// Its hand-off mark; zero until the frame was handed off (a frame that was submitted and not presented is never counted).
    t5: u64,
    /// The first of its two queries.
    first: u32,
}

/// What a frame that was handed off to the presentation engine reports to the recorder.
#[derive(Clone, Copy)]
pub(crate) struct HandedOff {
    pub(crate) counters: FrameCounters,
    pub(crate) table_binds: u64,
    pub(crate) present_id: Option<u32>,
}

/// The thread's frame-timing state.
pub(crate) struct Timing {
    /// The frame step's marks as they are taken.
    pub(crate) marks: FrameMarks,
    /// The frame step handed a frame off.
    pub(crate) handed_off: Option<HandedOff>,
    /// The built frame's counters, held from the build until the hand-off.
    pub(crate) built: Option<(FrameCounters, bool)>,
    wait: Option<GpuWait>,
    sets: u32,
    valid_bits: u32,
    period: f32,
    display: bool,
    next_present_id: u32,
    fences: [(u64, u32); FENCE_MEMORY],
    fence_next: usize,
}

impl Timing {
    pub(crate) fn new(valid_bits: u32, period: f32, display: bool) -> Self {
        Self {
            marks: FrameMarks::default(),
            handed_off: None,
            built: None,
            wait: None,
            sets: 0,
            valid_bits,
            period,
            display,
            next_present_id: 1,
            fences: [(0, 0); FENCE_MEMORY],
            fence_next: 0,
        }
    }

    /// The text the reading gives when GPU time is unavailable, if it is.
    pub(crate) fn gpu_unavailable(&self) -> Option<&'static str> {
        (self.valid_bits == 0).then_some("timestampValidBits 0")
    }

    /// The text the reading gives when presentation intervals are unavailable, if they are.
    pub(crate) fn presentation_unavailable(&self) -> Option<&'static str> {
        (!self.display).then_some("VK_GOOGLE_display_timing not advertised")
    }

    /// A frame step begins: its marks start at `t0`.
    pub(crate) fn begin(&mut self, t0: u64) {
        self.marks = FrameMarks {
            t0,
            ..FrameMarks::default()
        };
        self.handed_off = None;
        self.built = None;
    }

    /// The first query of the next frame's two (alternating between two sets), when the device has timestamps.
    pub(crate) fn next_queries(&mut self, has_timestamps: bool) -> Option<u32> {
        if !has_timestamps {
            return None;
        }
        let first = (self.sets % 2) * 2;
        self.sets = self.sets.wrapping_add(1);
        Some(first)
    }

    /// The frame submitted as `number` with its queries at `first` (none: no timestamps) awaits its GPU time.
    pub(crate) fn submitted(&mut self, number: u64, first: Option<u32>) {
        self.wait = first.map(|first| GpuWait {
            number,
            t5: 0,
            first,
        });
    }

    /// The `presentID` of the next present, when presentation timing is on, and the present fence it will be seen signalled on.
    pub(crate) fn present_id(&mut self, fence: u64) -> Option<u32> {
        if !self.display {
            return None;
        }
        let id = self.next_present_id;
        self.next_present_id = self.next_present_id.wrapping_add(1).max(1);
        // One mapping per fence, always the newest present's: a fence is reused by a later frame, and the older mapping must not
        // survive it (a lookup would find the old `presentID` and arm the wrong record's expiry).
        if let Some(entry) = self.fences.iter_mut().find(|(raw, _)| *raw == fence) {
            *entry = (fence, id);
        } else {
            self.fences[self.fence_next] = (fence, id);
            self.fence_next = (self.fence_next + 1) % FENCE_MEMORY;
        }
        Some(id)
    }

    /// The present fence `fence` was seen signalled: the `presentID` it carried.
    pub(crate) fn presentation_of(&self, fence: u64) -> Option<u32> {
        self.fences
            .iter()
            .find(|(raw, id)| *raw == fence && *id != 0)
            .map(|&(_, id)| id)
    }

    /// The frame was handed off at `t5`: its GPU time is owed to the window of `t5`.
    pub(crate) fn handed(&mut self, t5: u64, frame: HandedOff) {
        self.marks.t5 = t5;
        self.handed_off = Some(frame);
        if let Some(wait) = &mut self.wait {
            wait.t5 = t5;
        }
    }

    /// Reads the pending frame's GPU time when its proof is recorded (`proven`), and gives it to the recorder.
    pub(crate) fn poll_gpu(
        &mut self,
        gpu: &Gpu,
        metrics: &MetricsShared,
        proven: impl Fn(u64) -> bool,
    ) {
        let Some(wait) = self.wait else { return };
        if !proven(wait.number) {
            return;
        }
        self.wait = None;
        if wait.t5 == 0 {
            // Submitted and never handed off: no window owes it.
            return;
        }
        let nanoseconds = gpu
            .timestamp_results(wait.first)
            .and_then(|[start, end]| gpu_ns(start, end, self.valid_bits, self.period));
        metrics.recorder().gpu(wait.t5, nanoseconds);
        metrics.changed.notify_all();
    }

    /// Polls the presentation engine's feedback, at most [`BATCHES`] batches of [`BATCH`] records, and expires what is due.
    pub(crate) fn poll_presentation(&self, gpu: &Gpu, metrics: &MetricsShared) {
        if !self.presentation_pending(metrics) {
            return;
        }
        if let Some(chain) = gpu.current.as_ref() {
            drain_feedback(
                |records| gpu.past_presentation_timings(chain, records),
                |batch| {
                    let mut recorder = metrics.recorder();
                    for record in batch {
                        recorder
                            .presentation_resolved(record.present_id, record.actual_present_time);
                    }
                    drop(recorder);
                    metrics.changed.notify_all();
                },
            );
        }
        let now = metrics.now();
        let mut recorder = metrics.recorder();
        recorder.expire_due(now);
        drop(recorder);
        metrics.changed.notify_all();
    }

    /// Whether a present's feedback is still to come (the idle thread then wakes to poll).
    pub(crate) fn presentation_pending(&self, metrics: &MetricsShared) -> bool {
        self.display && metrics.recorder().has_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that holds `backlog` records and answers like `vkGetPastPresentationTimingGOOGLE`: up to the buffer's size per call,
    /// `VK_INCOMPLETE` while more remain.
    fn source(
        backlog: &std::cell::Cell<usize>,
    ) -> impl FnMut(&mut [vk::PastPresentationTimingGOOGLE]) -> Result<(usize, bool), vk::Result> + '_
    {
        move |records| {
            let written = backlog.get().min(records.len());
            for (index, record) in records[..written].iter_mut().enumerate() {
                record.present_id = u32::try_from(index).unwrap_or(0);
            }
            backlog.set(backlog.get() - written);
            Ok((written, backlog.get() > 0))
        }
    }

    /// A supplied answer of 20 records drains in three batches (8, 8, 4) of one poll; a 40-record backlog stops at 32 (four batches of
    /// 8) and the rest comes at the next poll.
    #[test]
    fn an_incomplete_answer_drains_in_bounded_batches_and_the_rest_waits_for_the_next_poll() {
        let backlog = std::cell::Cell::new(20);
        let mut sizes = Vec::new();
        let calls = drain_feedback(source(&backlog), |batch| sizes.push(batch.len()));
        assert_eq!((calls, sizes.as_slice()), (3, &[8, 8, 4][..]));
        assert_eq!(backlog.get(), 0);

        let backlog = std::cell::Cell::new(40);
        let mut delivered = 0;
        let calls = drain_feedback(source(&backlog), |batch| delivered += batch.len());
        assert_eq!(
            (calls, delivered),
            (4, 32),
            "four batches of eight, no more"
        );
        assert_eq!(backlog.get(), 8, "the rest is for the next poll");
        let calls = drain_feedback(source(&backlog), |batch| delivered += batch.len());
        assert_eq!((calls, delivered, backlog.get()), (1, 40, 0));
    }

    /// An empty answer ends the poll at once (a temporarily empty answer leaves everything pending), and a failure ends it too.
    #[test]
    fn an_empty_answer_or_a_failure_ends_the_poll_after_one_call() {
        let backlog = std::cell::Cell::new(0);
        let mut delivered = 0;
        assert_eq!(drain_feedback(source(&backlog), |_| delivered += 1), 1);
        assert_eq!(delivered, 0);
        let calls = drain_feedback(|_| Err(vk::Result::ERROR_DEVICE_LOST), |_| delivered += 1);
        assert_eq!((calls, delivered), (1, 0));
    }

    /// The expiry consequence: after the ring wraps, the fence of a present whose feedback never comes arms **that** present's expiry,
    /// not an older present's that shared the fence. A mapping that returned the older id would arm a record that is no longer
    /// pending, and the missing one would stay pending forever on an idle renderer.
    #[test]
    fn after_the_ring_wraps_a_reused_fence_arms_the_expiry_of_the_present_it_carries() {
        use crate::batch::FrameCounters;
        use crate::metrics::{MetricsWindow, Recorder};
        let mut timing = Timing::new(64, 1.0, true);
        let mut recorder = Recorder::default();
        recorder
            .windows
            .request(MetricsWindow {
                id: 1,
                start: 1,
                end: None,
            })
            .expect("requested");
        recorder.windows.boundary(0);
        let marks = |t: u64| FrameMarks {
            t0: t,
            t1: t + 1,
            t2: t + 2,
            t3: t + 3,
            t4: t + 4,
            t5: t + 5,
        };
        let mut hand_off = |timing: &mut Timing, fence: u64, t: u64, resolve: bool| {
            let id = timing.present_id(fence).expect("an id");
            recorder.drawn(&marks(t), Some(id), &FrameCounters::default(), 1);
            if resolve {
                recorder.presentation_resolved(id, t * 1_000);
            }
            id
        };
        // Five other fences first, so that the fence under test takes slot 5 of the ring of eight; two more, which take slots 6 and 7;
        // and then the fence is used again: the ring has wrapped, the new entry lands in slot 0, and the older entry (slot 5, a higher
        // index) is still in the table.
        for other in 1..=5_u64 {
            hand_off(&mut timing, other, 500 + other * 100, true);
        }
        let first = hand_off(&mut timing, 100, 1_000, true);
        for other in 6..=7_u64 {
            hand_off(&mut timing, other, 1_000 + other * 100, true);
        }
        // The fence is used again by a present whose feedback never arrives.
        let missing = hand_off(&mut timing, 100, 5_000, false);
        assert_ne!(missing, first);
        let carried = timing.presentation_of(100).expect("a mapping");
        assert_eq!(carried, missing, "the present the fence carries now");
        recorder.fence_seen(carried, 10_000);
        recorder.expire_due(10_000 + 999_999_999);
        assert!(recorder.has_pending(), "not yet a second");
        recorder.expire_due(10_000 + 1_000_000_000);
        assert!(!recorder.has_pending(), "the missing present expired");
        assert_eq!(recorder.totals.presentation_expired, 1);
    }

    /// A fence reused by a later frame maps to that frame's `presentID` alone, however many other fences were recorded in between
    /// (the ring of eight wraps): the older mapping does not survive it.
    #[test]
    fn a_reused_fence_maps_to_its_newest_present_after_the_ring_wraps() {
        let mut timing = Timing::new(64, 1.0, true);
        let first = timing.present_id(100).expect("an id");
        assert_eq!(timing.presentation_of(100), Some(first));
        // Seven other fences fill the ring behind it, then the fence is used again.
        for other in 1..=7_u64 {
            let _ = timing.present_id(other);
        }
        let again = timing.present_id(100).expect("an id");
        assert_ne!(again, first);
        assert_eq!(timing.presentation_of(100), Some(again), "the newest");
        // Wrap well past the ring with the same few fences alternating: the mapping is always the latest of each.
        let mut last = [0_u32; 3];
        for round in 0..40_usize {
            let which = round % 3;
            last[which] = timing.present_id(200 + which as u64).expect("an id");
        }
        for (which, id) in last.iter().enumerate() {
            assert_eq!(timing.presentation_of(200 + which as u64), Some(*id));
        }
        assert_eq!(timing.presentation_of(999), None, "an unknown fence");
    }
}
