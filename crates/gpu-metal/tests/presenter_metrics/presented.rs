//! When a case may stop a presenter whose subject is not the stop itself (CK-173), as a pure function of two counts so that the rule
//! and its message are checked with no window and no GPU (`hook_isolation.rs` includes this file by path).
//!
//! A drawable's presented handler runs when the display shows the drawable (or drops it). The presenter's teardown proves that every
//! one of them ran, against its one absolute deadline, so a stop issued while the latest drawable's presentation is still pending
//! ends `Unproven` whenever the display answers later than that deadline: after the very first drawable of a new window, while
//! the owner's activity delays the window server, for example. That is the teardown reporting a fact about the display, correctly,
//! and the cases that are about the teardown keep stopping with presentations pending on purpose. The cases that are about
//! something else first let the pending presentations arrive, bounded and with the window pumped, and fail there, named, when they do
//! not: a `Proven` end is then asserted exactly as before.

/// What the observer counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Drawables the presenter acquired.
    pub(crate) acquired: i64,
    /// Presented handlers that ran.
    pub(crate) answered: i64,
}

/// Whether the display has answered for every drawable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presentation {
    /// Every acquired drawable's presented handler ran.
    Answered,
    /// This many acquired drawables have not been answered.
    Pending(i64),
}

/// Judges one look at the counters.
pub(crate) fn presentation(counts: Counts) -> Presentation {
    let pending = counts.acquired - counts.answered;
    if pending > 0 {
        Presentation::Pending(pending)
    } else {
        Presentation::Answered
    }
}

/// What a case says when the wait ran out: it names the display, not the teardown.
pub(crate) fn never_answered(counts: Counts, waited: std::time::Duration) -> String {
    format!(
        "the display did not answer {} of {} acquired drawable(s) within {waited:?} (presented handlers run: {}); the case is waiting for the \
         presentation to arrive before it stops the presenter, so this is the presentation, not a teardown fault",
        counts.acquired - counts.answered,
        counts.acquired,
        counts.answered
    )
}
