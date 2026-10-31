//! The state GLFW's callbacks record into, and what one pump hands over from it.
//!
//! Callbacks only record, in constant work, and never call GLFW. This type is plain data with interior
//! mutability (the callbacks reach it through a shared reference from the window's user pointer, on the
//! main thread); the pump reads it after GLFW has returned.

use std::cell::Cell;

use ck_window::{NativeEvent, PixelSize};

/// What caused the close request that is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseSource {
    /// The window's close button (GLFW's close callback).
    Button,
    /// The Escape key.
    Key,
}

/// The pending state: the first close source in arrival order with a saturating count of the presses
/// behind it, the latest pixel size, whether the pixel size must be queried, and a needs-display flag.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    close: Cell<Option<CloseSource>>,
    absorbed: Cell<u32>,
    size: Cell<Option<(u32, u32)>>,
    query_size: Cell<bool>,
    display: Cell<bool>,
}

/// What one pump takes out of the pending state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Taken {
    pub(crate) close: Option<CloseSource>,
    pub(crate) absorbed: u32,
    pub(crate) size: Option<(u32, u32)>,
    pub(crate) query_size: bool,
    pub(crate) display: bool,
}

impl Pending {
    /// Records a close request. A request already waiting keeps its source; the press is counted
    /// (saturating) so storage stays constant however many presses one dispatch produces.
    pub(crate) fn record_close(&self, source: CloseSource) {
        if self.close.get().is_none() {
            self.close.set(Some(source));
        } else {
            self.absorbed.set(self.absorbed.get().saturating_add(1));
        }
    }

    /// Whether a close request is waiting.
    pub(crate) fn close_pending(&self) -> bool {
        self.close.get().is_some()
    }

    /// Records the latest framebuffer size (the callback's own values).
    pub(crate) fn record_size(&self, width: u32, height: u32) {
        self.size.set(Some((width, height)));
    }

    /// Records that the content scale changed: the pump reads the size once GLFW has returned.
    pub(crate) fn record_scale_changed(&self) {
        self.query_size.set(true);
    }

    /// Records that the window needs to be drawn again.
    pub(crate) fn record_display(&self) {
        self.display.set(true);
    }

    /// Whether anything is waiting to be handed over.
    pub(crate) fn is_pending(&self) -> bool {
        self.close.get().is_some()
            || self.size.get().is_some()
            || self.query_size.get()
            || self.display.get()
    }

    /// Takes everything and clears the state.
    pub(crate) fn take(&self) -> Taken {
        Taken {
            close: self.close.take(),
            absorbed: self.absorbed.take(),
            size: self.size.take(),
            query_size: self.query_size.take(),
            display: self.display.take(),
        }
    }
}

/// The wait a pump really makes: a blocking pump with state already waiting does not block.
pub(crate) fn effective_wait(wait: ck_window::Wait, pending: bool) -> ck_window::Wait {
    if pending { ck_window::Wait::Poll } else { wait }
}

/// The events a pump hands over, in order: at most one close, one pixel size, one needs-display. A zero
/// size (a minimised window) is a size like any other; the presenter suspends on it (design §8).
pub(crate) fn events(
    close: Option<CloseSource>,
    size: Option<PixelSize>,
    display: bool,
) -> Vec<NativeEvent> {
    let mut found = Vec::with_capacity(3);
    match close {
        Some(CloseSource::Button) => found.push(NativeEvent::CloseButton),
        Some(CloseSource::Key) => found.push(NativeEvent::EscapePressed),
        None => {}
    }
    if let Some(PixelSize { width, height }) = size {
        found.push(NativeEvent::PixelSizeChanged { width, height });
    }
    if display {
        found.push(NativeEvent::NeedsDisplay);
    }
    found
}

#[cfg(test)]
mod tests {
    use ck_window::Wait;

    use super::*;

    #[test]
    fn a_waiting_close_keeps_its_source_and_counts_the_presses_behind_it() {
        let pending = Pending::default();
        assert!(!pending.is_pending());
        pending.record_close(CloseSource::Key);
        pending.record_close(CloseSource::Button);
        pending.record_close(CloseSource::Key);
        assert!(pending.close_pending() && pending.is_pending());
        let taken = pending.take();
        assert_eq!(
            taken.close,
            Some(CloseSource::Key),
            "the first source in arrival order"
        );
        assert_eq!(taken.absorbed, 2);
        assert!(!pending.is_pending(), "taking clears everything");
        assert_eq!(pending.take().close, None);
    }

    #[test]
    fn the_count_saturates() {
        let pending = Pending::default();
        pending.absorbed.set(u32::MAX - 1);
        for _ in 0..5 {
            pending.record_close(CloseSource::Button);
        }
        assert_eq!(pending.take().absorbed, u32::MAX);
    }

    #[test]
    fn the_latest_size_wins_and_a_scale_change_asks_for_a_query() {
        let pending = Pending::default();
        pending.record_size(100, 80);
        pending.record_size(200, 160);
        pending.record_scale_changed();
        pending.record_scale_changed();
        let taken = pending.take();
        assert_eq!((taken.size, taken.query_size), (Some((200, 160)), true));
        assert_eq!(pending.take().size, None);
    }

    #[test]
    fn any_recorded_state_counts_as_pending_and_a_blocking_pump_does_not_block_on_it() {
        for record in [
            (|p: &Pending| p.record_close(CloseSource::Button)) as fn(&Pending),
            |p| p.record_size(1, 1),
            Pending::record_scale_changed,
            Pending::record_display,
        ] {
            let pending = Pending::default();
            assert_eq!(
                effective_wait(Wait::UntilEvent, pending.is_pending()),
                Wait::UntilEvent
            );
            record(&pending);
            assert!(pending.is_pending());
            assert_eq!(
                effective_wait(Wait::UntilEvent, pending.is_pending()),
                Wait::Poll
            );
        }
        assert_eq!(effective_wait(Wait::Poll, true), Wait::Poll);
        assert_eq!(effective_wait(Wait::Poll, false), Wait::Poll);
    }

    #[test]
    fn at_most_one_of_each_is_handed_over_in_a_fixed_order() {
        let size = Some(PixelSize {
            width: 0,
            height: 0,
        });
        assert_eq!(
            events(Some(CloseSource::Key), size, true),
            [
                NativeEvent::EscapePressed,
                NativeEvent::PixelSizeChanged {
                    width: 0,
                    height: 0
                },
                NativeEvent::NeedsDisplay
            ],
            "a zero size is reported as such"
        );
        assert_eq!(
            events(Some(CloseSource::Button), None, false),
            [NativeEvent::CloseButton]
        );
        assert!(events(None, None, false).is_empty());
    }
}
