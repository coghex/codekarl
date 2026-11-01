//! When a window's reading may be taken as final, and when an occluded window excuses it (CK-171), as pure functions of plain
//! values so that the rule is checked with no window and no GPU (`hook_isolation.rs` includes this file by path).
//!
//! A drawable's `presentedTime` is zero when its window is occluded, and a zero closes nothing: the presenter's reading of a window
//! whose latest shown drawable lies inside it stays not final (`closing_pending`) until a drawable is shown after its end or the
//! presenter ends. The owner's windows can cover a test window at any time, so a case that waits for a final reading must not wait for
//! what cannot come: when, and only when, the window was ever seen occluded and the reading is not final only because the closing
//! interval is open (`closing_pending`, nothing outstanding), it ends the presenter (whose proof accounts for every frame, so the
//! reading after it is exact whatever the poll could not show) and takes the final reading then. A window that was never seen occluded
//! keeps the full assertion.

/// What a case knows when it asks whether to stop waiting for a final reading.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts {
    /// The reading is the window's final one.
    pub(crate) complete: bool,
    /// The window's latest shown drawable lies inside it and none is shown at or after its end yet.
    pub(crate) closing_pending: bool,
    /// Frames of the window whose GPU time or presentation is still to come.
    pub(crate) outstanding: u64,
    /// Whether the window was ever seen without the visible bit.
    pub(crate) window: Window,
}

/// What the window's `occlusionState` has shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Window {
    /// It always had the visible bit.
    AlwaysVisible,
    /// At some time it did not (latched by the occlusion notification).
    SeenOccluded,
}

/// What to do about the reading.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    /// Final: every assertion holds as it always did.
    Final,
    /// Not final, and an occluded window is the reason: only the closing interval is outstanding. The case ends the presenter and
    /// reads the window then.
    Occluded,
    /// Keep waiting (and fail at the bound).
    Waiting,
}

/// Decides one poll.
pub(crate) fn settled(facts: &Facts) -> Settled {
    if facts.complete {
        Settled::Final
    } else if facts.window == Window::SeenOccluded
        && facts.closing_pending
        && facts.outstanding == 0
    {
        Settled::Occluded
    } else {
        Settled::Waiting
    }
}
