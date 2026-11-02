//! What the accumulated presenter reports say about the first frame, as a pure function of the reports, so
//! that its protocol is checked without a window or a GPU.
//!
//! The render thread publishes a frame (its count, its generation) before it enqueues `FirstFrameQueued`, so
//! a case that has seen the frame has not yet seen the report. A case waits for the report it asserts, keeps
//! every report it drains, and checks the whole history once more at the proven end.

use ck_gpu_vulkan::PresenterReport;

/// The state of the first frame's report in a history of drained reports.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FirstFrame {
    /// No report yet: the frame may be drawn, but its report is not queued.
    Pending,
    /// `FirstFrameQueued` exactly once and nothing else.
    Reported,
    /// The presenter reported a failure (the messages, in order).
    Failed(Vec<String>),
    /// `FirstFrameQueued` more than once (how many times).
    Duplicated(usize),
}

/// Reads the history of every report drained so far.
pub(crate) fn first_frame(seen: &[PresenterReport]) -> FirstFrame {
    let failures: Vec<String> = seen
        .iter()
        .filter_map(|report| match report {
            PresenterReport::Failed { message } => Some(message.clone()),
            PresenterReport::FirstFrameQueued => None,
        })
        .collect();
    if !failures.is_empty() {
        return FirstFrame::Failed(failures);
    }
    match seen.len() {
        0 => FirstFrame::Pending,
        1 => FirstFrame::Reported,
        many => FirstFrame::Duplicated(many),
    }
}

/// `Ok` when the history is the one first-frame report; the other states say what they are.
pub(crate) fn exactly_once(seen: &[PresenterReport]) -> Result<(), String> {
    match first_frame(seen) {
        FirstFrame::Reported => Ok(()),
        FirstFrame::Pending => Err("no first-frame report".to_owned()),
        FirstFrame::Failed(messages) => Err(format!("the presenter failed: {messages:?}")),
        FirstFrame::Duplicated(count) => Err(format!("{count} first-frame reports, not one")),
    }
}
