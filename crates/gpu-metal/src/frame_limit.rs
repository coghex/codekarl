//! The frame-rate limit's one decision (CK-313).
//!
//! Given when the last frame started, the time now and the limit, a frame may start now or at a stated time. The render
//! thread only follows the answer: it reads the clock, asks, and waits until the stated time on the signal it already waits
//! on, so a stop, a texture wake or a changed limit still ends the wait. The decision touches no clock and no thread, so its
//! tests give the times.

use std::time::{Duration, Instant};

/// When a wanted frame may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameStart {
    /// At once.
    Now,
    /// Not before this time.
    At(Instant),
}

/// The least time from the start of one frame to the start of the next under a limit of `fps` frames per second.
///
/// The interval is whole nanoseconds, rounded down, so a limit never asks for more than its share of a second.
fn interval(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1)))
}

/// Whether a frame wanted at `now` may start, given the `start` of the frame before it (`None`: no frame was started yet) and
/// the `limit` in frames per second (`None`: no limit, a frame may always start).
///
/// With a limit of N, no frame starts sooner than one N-th of a second after the start of the frame before. A frame wanted
/// exactly when that time comes may start; one wanted later may start at once. The decision is made again for each look, so a
/// changed limit applies from the next one.
pub(crate) fn decide(start: Option<Instant>, now: Instant, limit: Option<u32>) -> FrameStart {
    let (Some(start), Some(fps)) = (start, limit) else {
        return FrameStart::Now;
    };
    let due = start + interval(fps);
    if now >= due {
        FrameStart::Now
    } else {
        FrameStart::At(due)
    }
}

#[cfg(test)]
mod tests {
    //! Every limit run here is 30 or 60 (the owner's rule for this slice): the decision is arithmetic on given times, with no
    //! sleep, no window and no GPU.

    use super::*;

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    #[test]
    fn the_first_frame_may_start_at_once_whatever_the_limit() {
        let now = Instant::now();
        assert_eq!(decide(None, now, Some(30)), FrameStart::Now);
        assert_eq!(decide(None, now, Some(60)), FrameStart::Now);
        assert_eq!(decide(None, now, None), FrameStart::Now);
    }

    #[test]
    fn a_frame_wanted_inside_the_interval_waits_for_exactly_its_end() {
        let base = Instant::now();
        let thirty = decide(Some(base), at(base, 10), Some(30));
        assert_eq!(thirty, FrameStart::At(base + interval(30)));
        let sixty = decide(Some(base), at(base, 10), Some(60));
        assert_eq!(sixty, FrameStart::At(base + interval(60)));
        // The intervals are one 30th and one 60th of a second, to the nanosecond.
        assert_eq!(interval(30), Duration::from_nanos(33_333_333));
        assert_eq!(interval(60), Duration::from_nanos(16_666_666));
    }

    #[test]
    fn a_frame_may_start_exactly_when_the_interval_has_passed_and_not_before() {
        let base = Instant::now();
        for fps in [30, 60] {
            let due = base + interval(fps);
            assert_eq!(
                decide(
                    Some(base),
                    base + interval(fps).saturating_sub(Duration::from_nanos(1)),
                    Some(fps)
                ),
                FrameStart::At(due),
                "one nanosecond early at {fps}"
            );
            assert_eq!(
                decide(Some(base), due, Some(fps)),
                FrameStart::Now,
                "exactly due at {fps}"
            );
        }
    }

    #[test]
    fn a_frame_wanted_after_the_interval_starts_at_once() {
        let base = Instant::now();
        assert_eq!(decide(Some(base), at(base, 40), Some(30)), FrameStart::Now);
        assert_eq!(decide(Some(base), at(base, 20), Some(60)), FrameStart::Now);
        assert_eq!(
            decide(Some(base), at(base, 5_000), Some(30)),
            FrameStart::Now
        );
    }

    #[test]
    fn several_wants_inside_one_interval_are_one_frame_at_one_time() {
        // Wants coalesce into the one dirty flag, so every look inside the interval is answered with the same time, and the
        // frame that starts then moves the start the next interval counts from.
        let base = Instant::now();
        let first = decide(Some(base), at(base, 2), Some(30));
        let second = decide(Some(base), at(base, 9), Some(30));
        let third = decide(Some(base), at(base, 20), Some(30));
        assert_eq!(first, FrameStart::At(base + interval(30)));
        assert_eq!(second, first);
        assert_eq!(third, first);
        let FrameStart::At(due) = first else {
            panic!("a frame wanted early waits")
        };
        // The frame that starts at `due` is the next one's reference.
        assert_eq!(
            decide(Some(due), at(base, 40), Some(30)),
            FrameStart::At(due + interval(30))
        );
    }

    #[test]
    fn without_a_limit_every_wanted_frame_may_start_at_once() {
        let base = Instant::now();
        assert_eq!(decide(Some(base), base, None), FrameStart::Now);
        assert_eq!(decide(Some(base), at(base, 1), None), FrameStart::Now);
    }

    #[test]
    fn a_changed_limit_changes_the_decision_from_the_next_look() {
        // none -> 30 -> 60 -> none, with no restart: the same last start and now, asked again under each limit.
        let base = Instant::now();
        let now = at(base, 20);
        assert_eq!(decide(Some(base), now, None), FrameStart::Now);
        assert_eq!(
            decide(Some(base), now, Some(30)),
            FrameStart::At(base + interval(30))
        );
        assert_eq!(decide(Some(base), now, Some(60)), FrameStart::Now);
        assert_eq!(decide(Some(base), now, None), FrameStart::Now);
    }
}
