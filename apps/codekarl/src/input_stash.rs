//! What the UI VM has not yet been given of the input (milestone 8, slice 0; `docs/ui-design.md` §3.12).
//!
//! The map and the time controls take the input at the loop's input tick; the UI VM's `frame_update` callback is a slower, optional
//! consumer. The stash is the bounded share of the UI between two accepted posts:
//!
//! - **per action:** the latest `held`, with `pressed` and `released` OR-ed since the last accepted delivery;
//! - **transitions:** ordered, at most [`TRANSITION_CAPACITY`]; when full the oldest is dropped and `overflow` is set;
//! - **pointer:** the latest position, left-button held state and hit key, plus ordered events; only adjacent moves with the same key
//!   coalesce, and the oldest event is dropped past the bound with `pointer_overflow` set;
//! - **typed entries** (CK-314): one ordered list, at most [`TYPED_CAPACITY`]; past the bound the later entries are dropped (the earlier
//!   ones keep their order) and `typed_lost` is set. A snapshot of a newer text-input epoch discards what was gathered before it;
//!   the epoch is kept (like `held`) so the UI can tell which entries came after its own call;
//! - **`overflow`** and **`reset`:** OR-ed (a snapshot's own `overflow` is OR-ed in).
//!
//! The stash is cleared only when the UI VM accepted the post that carried it, and a clear keeps each action's latest `held`: that is
//! the current state, not an event.

use std::collections::{BTreeMap, VecDeque};

use ck_window::{
    InputSnapshot, PointerPosition, TRANSITION_CAPACITY, TYPED_CAPACITY, Transition, TypedEntry,
};

pub(crate) const POINTER_EVENT_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PointerView {
    pub(crate) position: PointerPosition,
    pub(crate) held: bool,
    pub(crate) over: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PointerViewEvent {
    pub(crate) kind: &'static str,
    pub(crate) position: PointerPosition,
    pub(crate) key: Option<String>,
    /// Whether the position lay inside one of `key`'s published rectangles (translated and clipped), in the regions the router used at
    /// the tick that routed the event. Always false without a key.
    pub(crate) inside: bool,
    pub(crate) button: Option<&'static str>,
    pub(crate) amount: Option<i32>,
    pub(crate) unit: Option<&'static str>,
}

/// One action's share of the stash.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StashedAction {
    pub(crate) held: bool,
    pub(crate) pressed: bool,
    pub(crate) released: bool,
}

/// The input the UI has not been given yet.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the stash's latched conditions are independent flags"
)]
pub(crate) struct InputStash {
    actions: BTreeMap<String, StashedAction>,
    transitions: VecDeque<Transition>,
    overflow: bool,
    reset: bool,
    pointer: Option<PointerView>,
    pointer_events: VecDeque<PointerViewEvent>,
    pointer_overflow: bool,
    typed: VecDeque<TypedEntry>,
    typed_lost: bool,
    typed_epoch: u64,
    capture: Option<ck_window::CaptureResult>,
}

impl InputStash {
    /// Adds a tick's input: the latest `held`, the OR-ed latches, the ordered transitions (the oldest dropped past the bound, which
    /// sets `overflow`) and the OR-ed `overflow` and `reset`.
    pub(crate) fn absorb(&mut self, input: &InputSnapshot) {
        for action in &input.actions {
            let stashed = self.actions.entry(action.name.clone()).or_default();
            stashed.held = action.held;
            stashed.pressed |= action.pressed;
            stashed.released |= action.released;
        }
        for transition in &input.transitions {
            if self.transitions.len() == TRANSITION_CAPACITY {
                self.transitions.pop_front();
                self.overflow = true;
            }
            self.transitions.push_back(transition.clone());
        }
        self.overflow |= input.overflow;
        self.reset |= input.reset;
        if input.typed_epoch > self.typed_epoch {
            // Text input was turned on or off since: what was gathered before was typed under the old setting.
            self.typed.clear();
            self.typed_lost = false;
            self.typed_epoch = input.typed_epoch;
        }
        for entry in &input.typed {
            if self.typed.len() == TYPED_CAPACITY {
                self.typed_lost = true;
                break;
            }
            self.typed.push_back(*entry);
        }
        self.typed_lost |= input.typed_lost;
        if input.capture.is_some() {
            self.capture.clone_from(&input.capture);
        }
    }

    /// Drops everything that is an event: transitions, latches, `reset` and `overflow`. Each action's latest `held` stays.
    pub(crate) fn clear(&mut self) {
        for stashed in self.actions.values_mut() {
            stashed.pressed = false;
            stashed.released = false;
        }
        self.transitions.clear();
        self.overflow = false;
        self.reset = false;
        self.pointer_events.clear();
        self.pointer_overflow = false;
        self.typed.clear();
        self.typed_lost = false;
        self.capture = None;
    }

    /// The tick's input while no callback is registered: nothing of it is kept but each action's latest `held`.
    pub(crate) fn follow_held(&mut self, input: &InputSnapshot) {
        self.clear();
        self.typed_epoch = self.typed_epoch.max(input.typed_epoch);
        self.capture.clone_from(&input.capture);
        for action in &input.actions {
            self.actions.entry(action.name.clone()).or_default().held = action.held;
        }
        if let Some(position) = input.pointer_position {
            let over = self.pointer.as_ref().and_then(|old| old.over.clone());
            self.pointer = Some(PointerView {
                position,
                held: input.pointer_held,
                over,
            });
        } else if let Some(pointer) = &mut self.pointer {
            pointer.held = input.pointer_held;
        }
    }

    pub(crate) fn absorb_pointer(
        &mut self,
        latest: Option<PointerView>,
        events: impl IntoIterator<Item = PointerViewEvent>,
        pointer_overflow: bool,
    ) {
        if let Some(latest) = latest {
            self.pointer = Some(latest);
        }
        for event in events {
            if event.kind == "move"
                && let Some(previous) = self.pointer_events.back_mut()
                && previous.kind == "move"
                && previous.key == event.key
            {
                *previous = event;
                continue;
            }
            if self.pointer_events.len() == POINTER_EVENT_CAPACITY {
                self.pointer_events.pop_front();
                self.pointer_overflow = true;
            }
            self.pointer_events.push_back(event);
        }
        self.pointer_overflow |= pointer_overflow;
    }

    /// Each action as `(name, state)`, in name order.
    pub(crate) fn actions(&self) -> impl Iterator<Item = (&str, StashedAction)> {
        self.actions
            .iter()
            .map(|(name, stashed)| (name.as_str(), *stashed))
    }

    /// The ordered transitions, oldest first.
    pub(crate) fn transitions(&self) -> impl Iterator<Item = &Transition> {
        self.transitions.iter()
    }

    pub(crate) fn overflow(&self) -> bool {
        self.overflow
    }

    pub(crate) fn reset(&self) -> bool {
        self.reset
    }

    pub(crate) fn pointer(&self) -> Option<&PointerView> {
        self.pointer.as_ref()
    }

    pub(crate) fn pointer_events(&self) -> impl Iterator<Item = &PointerViewEvent> {
        self.pointer_events.iter()
    }

    pub(crate) fn pointer_overflow(&self) -> bool {
        self.pointer_overflow
    }

    /// The typed entries, in the order typed.
    pub(crate) fn typed(&self) -> impl Iterator<Item = &TypedEntry> {
        self.typed.iter()
    }

    pub(crate) fn typed_lost(&self) -> bool {
        self.typed_lost
    }

    pub(crate) fn typed_epoch(&self) -> u64 {
        self.typed_epoch
    }

    pub(crate) fn capture(&self) -> Option<&ck_window::CaptureResult> {
        self.capture.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use ck_window::{ActionSnapshot, Edge, PointerPosition};

    use super::*;

    fn action(name: &str, held: bool, pressed: bool, released: bool) -> ActionSnapshot {
        ActionSnapshot {
            name: name.to_owned(),
            held,
            pressed,
            released,
        }
    }

    fn snapshot(actions: Vec<ActionSnapshot>, transitions: Vec<Transition>) -> InputSnapshot {
        InputSnapshot {
            sequence: 0,
            actions,
            transitions,
            overflow: false,
            reset: false,
            overflow_count: 0,
            pointer_events: Vec::new(),
            pointer_position: None,
            pointer_held: false,
            pointer_overflow: false,
            typed: Vec::new(),
            typed_lost: false,
            typed_epoch: 0,
            capture: None,
        }
    }

    fn edge(name: &str, edge: Edge) -> Transition {
        Transition {
            action: name.to_owned(),
            edge,
        }
    }

    #[test]
    fn absorbing_keeps_the_latest_held_ors_the_latches_and_orders_the_transitions() {
        let mut stash = InputStash::default();
        stash.absorb(&snapshot(
            vec![action("a", true, true, false)],
            vec![edge("a", Edge::Press)],
        ));
        stash.absorb(&snapshot(
            vec![action("a", false, false, true)],
            vec![edge("a", Edge::Release)],
        ));
        let actions: Vec<_> = stash.actions().collect();
        assert_eq!(
            actions,
            [(
                "a",
                StashedAction {
                    held: false,
                    pressed: true,
                    released: true
                }
            )]
        );
        let edges: Vec<_> = stash.transitions().map(|t| t.edge).collect();
        assert_eq!(edges, [Edge::Press, Edge::Release]);
        assert!(!stash.overflow() && !stash.reset());
    }

    #[test]
    fn past_the_bound_the_oldest_transitions_go_and_overflow_is_set() {
        let mut stash = InputStash::default();
        let many: Vec<Transition> = (0..TRANSITION_CAPACITY + 44)
            .map(|index| edge(&format!("t{index:03}"), Edge::Press))
            .collect();
        stash.absorb(&snapshot(vec![action("a", true, false, false)], many));
        assert!(stash.overflow());
        let kept: Vec<_> = stash.transitions().map(|t| t.action.clone()).collect();
        assert_eq!(kept.len(), TRANSITION_CAPACITY);
        assert_eq!(kept[0], "t044", "the oldest 44 were dropped");
        assert_eq!(kept.last().map(String::as_str), Some("t299"));
    }

    #[test]
    fn a_snapshots_own_overflow_and_reset_are_ored_in_and_a_clear_keeps_only_held() {
        let mut stash = InputStash::default();
        let mut input = snapshot(
            vec![action("a", true, true, true)],
            vec![edge("a", Edge::Press)],
        );
        input.overflow = true;
        input.reset = true;
        stash.absorb(&input);
        assert!(stash.overflow() && stash.reset());
        stash.clear();
        assert!(!stash.overflow() && !stash.reset());
        assert_eq!(stash.transitions().count(), 0);
        assert_eq!(
            stash.actions().collect::<Vec<_>>(),
            [(
                "a",
                StashedAction {
                    held: true,
                    pressed: false,
                    released: false
                }
            )]
        );
    }

    #[test]
    fn following_held_keeps_only_the_current_state_of_the_tick() {
        let mut stash = InputStash::default();
        stash.absorb(&snapshot(
            vec![action("a", true, true, false)],
            vec![edge("a", Edge::Press)],
        ));
        stash.follow_held(&snapshot(
            vec![
                action("a", false, false, true),
                action("b", true, true, false),
            ],
            vec![edge("a", Edge::Release), edge("b", Edge::Press)],
        ));
        assert_eq!(stash.transitions().count(), 0);
        assert_eq!(
            stash.actions().collect::<Vec<_>>(),
            [
                ("a", StashedAction::default()),
                (
                    "b",
                    StashedAction {
                        held: true,
                        pressed: false,
                        released: false
                    }
                ),
            ]
        );
    }

    #[test]
    fn pointer_moves_coalesce_and_the_bounded_queue_keeps_the_newest_events() {
        let mut stash = InputStash::default();
        let move_event = |x| PointerViewEvent {
            kind: "move",
            position: PointerPosition { x, y: 0 },
            key: Some("panel".to_owned()),
            inside: false,
            button: None,
            amount: None,
            unit: None,
        };
        let mut moves = (0..2).map(move_event);
        stash.absorb_pointer(None, moves.by_ref(), false);
        stash.absorb_pointer(
            None,
            [PointerViewEvent {
                kind: "button_down",
                position: PointerPosition { x: 2, y: 0 },
                key: Some("panel".to_owned()),
                inside: true,
                button: Some("middle"),
                amount: None,
                unit: None,
            }],
            false,
        );
        stash.absorb_pointer(None, (2..4).map(move_event), false);
        assert_eq!(stash.pointer_events.len(), 3);
        assert_eq!(stash.pointer_events[0].position.x, 1);
        assert_eq!(stash.pointer_events[1].kind, "button_down");
        assert_eq!(stash.pointer_events[2].position.x, 3);

        let event = |x| PointerViewEvent {
            kind: "down",
            position: PointerPosition { x, y: 0 },
            key: None,
            inside: false,
            button: None,
            amount: None,
            unit: None,
        };
        stash.absorb_pointer(None, (0..70).map(event), false);
        assert_eq!(stash.pointer_events.len(), POINTER_EVENT_CAPACITY);
        assert_eq!(stash.pointer_events.front().unwrap().position.x, 6);
        assert_eq!(stash.pointer_events.back().unwrap().position.x, 69);
        assert!(stash.pointer_overflow());
    }

    #[test]
    fn unregistered_pointer_state_is_kept_but_events_and_overflow_are_dropped() {
        let mut stash = InputStash::default();
        let latest = PointerView {
            position: PointerPosition { x: 256, y: 512 },
            held: true,
            over: Some("readout".to_owned()),
        };
        stash.absorb_pointer(
            Some(latest.clone()),
            [PointerViewEvent {
                kind: "down",
                position: latest.position,
                key: Some("readout".to_owned()),
                inside: false,
                button: None,
                amount: None,
                unit: None,
            }],
            true,
        );
        let mut current = snapshot(vec![action("a", true, false, false)], Vec::new());
        current.pointer_held = true;
        stash.follow_held(&current);
        assert_eq!(stash.pointer(), Some(&latest));
        assert_eq!(stash.pointer_events().count(), 0);
        assert!(!stash.pointer_overflow());
    }

    fn typed_snapshot(entries: &[TypedEntry], lost: bool, epoch: u64) -> InputSnapshot {
        let mut input = snapshot(Vec::new(), Vec::new());
        input.typed = entries.to_vec();
        input.typed_lost = lost;
        input.typed_epoch = epoch;
        input
    }

    #[test]
    fn typed_entries_gather_in_order_are_bounded_with_the_later_dropped_and_cleared_with_the_rest()
    {
        let mut stash = InputStash::default();
        stash.absorb(&typed_snapshot(&[TypedEntry::Char('a')], false, 0));
        stash.absorb(&typed_snapshot(
            &[TypedEntry::Backspace, TypedEntry::Char('b')],
            false,
            0,
        ));
        assert_eq!(
            stash.typed().copied().collect::<Vec<_>>(),
            [
                TypedEntry::Char('a'),
                TypedEntry::Backspace,
                TypedEntry::Char('b')
            ]
        );
        assert!(!stash.typed_lost());
        // Past the bound the later entries are dropped and the loss is set; the earlier ones keep their order.
        let many = vec![TypedEntry::Tab; TYPED_CAPACITY];
        stash.absorb(&typed_snapshot(&many, false, 0));
        assert_eq!(stash.typed().count(), TYPED_CAPACITY);
        assert!(stash.typed_lost());
        assert_eq!(stash.typed().next(), Some(&TypedEntry::Char('a')));
        // A snapshot's own loss is OR-ed in, and a clear (an accepted post) drops entries and the loss.
        stash.clear();
        assert_eq!(stash.typed().count(), 0);
        assert!(!stash.typed_lost());
        stash.absorb(&typed_snapshot(&[], true, 0));
        assert!(stash.typed_lost());
    }

    #[test]
    fn a_newer_text_input_epoch_discards_what_was_gathered_before_it_and_the_epoch_survives_a_clear()
     {
        let mut stash = InputStash::default();
        stash.absorb(&typed_snapshot(&[TypedEntry::Char('a')], true, 1));
        stash.absorb(&typed_snapshot(&[TypedEntry::Char('b')], false, 2));
        assert_eq!(
            stash.typed().copied().collect::<Vec<_>>(),
            [TypedEntry::Char('b')]
        );
        assert!(
            !stash.typed_lost(),
            "the earlier loss belonged to the older epoch"
        );
        assert_eq!(stash.typed_epoch(), 2);
        stash.clear();
        assert_eq!(stash.typed_epoch(), 2, "the epoch is state, like `held`");
        // With no callback registered the entries are dropped, and the epoch still follows.
        stash.follow_held(&typed_snapshot(&[TypedEntry::Char('c')], false, 3));
        assert_eq!(stash.typed().count(), 0);
        assert_eq!(stash.typed_epoch(), 3);
    }
}
