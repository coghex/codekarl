//! The UI link and the shared UI VM handle (design §4, §5).
//!
//! **The handle is shared on the main thread only** ([`UiVm`], an `Rc<RefCell<VmOwner>>`): the runtime's
//! owner adapter (`vm::VmAdapter`) uses it during the start and stop phases and the run closure (through
//! [`CodekarlUi`]) during the run phase, never at the same time, and no other thread ever touches it.
//!
//! [`CodekarlUi`] adapts `ck-lua`'s posted calls to `ck-window`'s [`UiLink`]:
//!
//! - `Opened` is a posted call `opened` with no arguments, `CloseRequested { source }` is `close_request`
//!   with `"button"` or `"key"`;
//! - `Full` and a disconnection map to [`PostError::Full`] and [`PostError::Disconnected`];
//! - a `close_request` result `Returned` whose first value is `true` is [`Answer::Accept`], any other
//!   `Returned` is [`Answer::Decline`], `NotRegistered` is [`Answer::NotRegistered`] and `Failed` is
//!   [`Answer::Failed`] with the message `ui callback <name> failed: <message>` and the traceback (the same
//!   for `opened`, whose `Accept` and `Decline` the loop ignores);
//! - **`is_disconnected()`** (CK-62 §7.3) is true once `take_failure` has returned a failure (a host panic
//!   or a panic, which send no call result, or an error ended by cancellation: it is remembered and
//!   [`CodekarlUi::failure`] gives its text), once the handle is Ending or Finished, or once the handle
//!   reported a disconnection. The loop asks it on every pass, after taking answers and before deciding
//!   whether to wait; because the owner thread sends its terminal report before its wakes (CK-66 §2), a
//!   check made after any wake sees it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use std::collections::BTreeMap;

use ck_lua::{CallId, CallOutcome, ConfigValue, VmError, VmOwner, VmState};
use ck_window::{Answer, CloseSource, Edge, PostError, UiAnswer, UiEvent, UiLink, UpdateEvent};

use crate::vm::UI_CALL_LIMIT;
use crate::window_api::{CLOSE_REQUEST, FRAME_UPDATE, OPENED};

/// The shared handle of the UI VM.
pub(crate) type UiVm = Rc<RefCell<VmOwner>>;

/// Where the UI VM's spawn closure puts its handle, for the run closure to find (empty when there is no
/// `ui` VM, or before the VM is spawned).
pub(crate) type UiSlot = Rc<RefCell<Option<UiVm>>>;

/// The callback name and arguments of an event.
///
/// # Errors
///
/// An update whose sequence is not a Lua integer (above `i64::MAX`): the call is not built, so no callback is ever given
/// an altered identity.
fn call_of(event: &UiEvent) -> Result<(&'static str, Vec<ConfigValue>), String> {
    Ok(match event {
        UiEvent::Opened => (OPENED, Vec::new()),
        UiEvent::CloseRequested { source } => (
            CLOSE_REQUEST,
            vec![ConfigValue::String(
                match source {
                    CloseSource::Button => "button",
                    CloseSource::Key => "key",
                }
                .to_owned(),
            )],
        ),
        UiEvent::Update(update) => (FRAME_UPDATE, vec![update_table(update)?]),
    })
}

/// A window size as the integer a script sees (a `u32` always fits, so the saturating end is unreachable).
fn integer(value: impl TryInto<i64>) -> ConfigValue {
    ConfigValue::Integer(value.try_into().unwrap_or(i64::MAX))
}

fn boolean(value: bool) -> ConfigValue {
    ConfigValue::Bool(value)
}

/// The table a `frame_update` callback is given (design §8.2): `dt`, `elapsed` (seconds), `sequence`; the window in
/// points (`width`, `height`) and in pixels (`pixel_width`, `pixel_height`); `actions`, one entry per action name of
/// the run's bindings, unbound ones included, each `{held, pressed, released}`; `transitions`, the ordered edges as
/// `{action = name, kind = "press" | "release"}`; and `overflow`, `reset`. Built from `ConfigValue` only (no `mlua`
/// type), in O(actions + transitions), both bounded by the input snapshot.
///
/// The sequence is a `u64` and a script's integer an `i64`; a sequence above `i64::MAX` is an error (never saturated or
/// wrapped), which the link turns into a terminal failure.
fn update_table(update: &UpdateEvent) -> Result<ConfigValue, String> {
    let sequence = i64::try_from(update.sequence).map_err(|_| {
        format!(
            "frame_update sequence {} does not fit a Lua integer (at most {})",
            update.sequence,
            i64::MAX
        )
    })?;
    let actions = update
        .input
        .actions
        .iter()
        .map(|action| {
            (
                action.name.clone(),
                ConfigValue::Table(BTreeMap::from([
                    ("held".to_owned(), boolean(action.held)),
                    ("pressed".to_owned(), boolean(action.pressed)),
                    ("released".to_owned(), boolean(action.released)),
                ])),
            )
        })
        .collect();
    let transitions = update
        .input
        .transitions
        .iter()
        .map(|transition| {
            ConfigValue::Table(BTreeMap::from([
                (
                    "action".to_owned(),
                    ConfigValue::String(transition.action.clone()),
                ),
                (
                    "kind".to_owned(),
                    ConfigValue::String(
                        match transition.edge {
                            Edge::Press => "press",
                            Edge::Release => "release",
                        }
                        .to_owned(),
                    ),
                ),
            ]))
        })
        .collect();
    Ok(ConfigValue::Table(BTreeMap::from([
        ("dt".to_owned(), ConfigValue::Float(update.dt.as_secs_f64())),
        (
            "elapsed".to_owned(),
            ConfigValue::Float(update.elapsed.as_secs_f64()),
        ),
        ("sequence".to_owned(), ConfigValue::Integer(sequence)),
        ("width".to_owned(), integer(update.metrics.points.0)),
        ("height".to_owned(), integer(update.metrics.points.1)),
        (
            "pixel_width".to_owned(),
            integer(update.metrics.pixels.width),
        ),
        (
            "pixel_height".to_owned(),
            integer(update.metrics.pixels.height),
        ),
        ("actions".to_owned(), ConfigValue::Table(actions)),
        ("transitions".to_owned(), ConfigValue::Array(transitions)),
        ("overflow".to_owned(), boolean(update.input.overflow)),
        ("reset".to_owned(), boolean(update.input.reset)),
    ])))
}

/// The name of an event's callback.
fn name_of(event: &UiEvent) -> &'static str {
    match event {
        UiEvent::Opened => OPENED,
        UiEvent::CloseRequested { .. } => CLOSE_REQUEST,
        UiEvent::Update(_) => FRAME_UPDATE,
    }
}

/// The answer a call's outcome gives.
pub(crate) fn answer_of(event: &UiEvent, outcome: CallOutcome) -> Answer {
    match outcome {
        // An update's results are ignored: any return completes it.
        CallOutcome::Returned(_) if matches!(event, UiEvent::Update(_)) => Answer::Returned,
        CallOutcome::Returned(values) => {
            if matches!(values.first(), Some(Some(ConfigValue::Bool(true)))) {
                Answer::Accept
            } else {
                Answer::Decline
            }
        }
        CallOutcome::NotRegistered => Answer::NotRegistered,
        CallOutcome::Failed { message, traceback } => Answer::Failed {
            message: format!(
                "ui callback {} failed: {message}\n{traceback}",
                name_of(event)
            ),
        },
    }
}

/// The text of a terminal failure of the UI VM.
fn describe(error: VmError) -> String {
    match crate::vm::owner_error(error) {
        ck_runtime::OwnerError::Failed { message } => message,
        ck_runtime::OwnerError::Deadline => "deadline missed".to_owned(),
    }
}

/// What the link remembers between passes.
#[derive(Default)]
struct State {
    /// The calls posted and not yet answered, oldest first (results arrive in the order the calls ran).
    pending: RefCell<VecDeque<(CallId, UiEvent)>>,
    /// The `game_time` calls posted and not yet answered (their results are taken and not given to the window loop).
    times: RefCell<Vec<CallId>>,
    /// The `game_hover` calls posted and not yet answered.
    hovers: RefCell<Vec<CallId>>,
    /// The snapshot last delivered through `game_time`, to post the next only when something displayed differs.
    delivered: Cell<Option<crate::game::slot::SimSnapshot>>,
    /// Hover last posted; outer `None` means callback registration changed and needs a fresh delivery.
    delivered_hover: RefCell<DeliveredHover>,
    /// Which latest-wins channel gets the next free non-lifecycle call place.
    next_update: Cell<UpdateChannel>,
    /// The text of the terminal failure `take_failure` returned, once.
    failure: RefCell<Option<String>>,
    /// A post reported that the VM ended.
    disconnected: Cell<bool>,
}

#[derive(Default)]
enum DeliveredHover {
    #[default]
    Unsent,
    Sent(Option<crate::map_app::Hover>),
}

#[derive(Clone, Copy, Default)]
enum UpdateChannel {
    #[default]
    Time,
    Hover,
}

/// The UI link: cheap to clone, and every clone shares the handle and what the link remembers, so the
/// run closure keeps one for the late-failure steps while `ck_window::run` owns another.
#[derive(Clone)]
pub(crate) struct CodekarlUi {
    slot: UiSlot,
    state: Rc<State>,
    map: Option<crate::map_app::SharedController>,
    sim: Option<SimLink>,
}

/// The simulation owner as the UI link sees it: where presses go, and what it published.
#[derive(Clone)]
pub(crate) struct SimLink {
    pub(crate) controls: crate::game::controls::SimControls,
    pub(crate) slot: std::sync::Arc<crate::game::slot::SimSlot>,
    /// Whether the UI VM has registered a `game.on_time` callback.
    pub(crate) time_registered: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether the UI VM has registered a `game.on_hover` callback.
    pub(crate) hover_registered: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CodekarlUi {
    /// A link over whatever handle `slot` will hold.
    pub(crate) fn new(slot: UiSlot) -> Self {
        Self {
            slot,
            state: Rc::new(State::default()),
            map: None,
            sim: None,
        }
    }

    pub(crate) fn with_map(slot: UiSlot, map: crate::map_app::SharedController) -> Self {
        Self {
            slot,
            state: Rc::new(State::default()),
            map: Some(map),
            sim: None,
        }
    }

    /// This link with the simulation owner attached: it carries the update's time presses to the owner, and ends the run when
    /// the owner records a terminal failure.
    pub(crate) fn with_sim(mut self, sim: SimLink) -> Self {
        self.sim = Some(sim);
        self
    }

    /// Whether the simulation owner recorded a terminal failure; the message becomes the link's own terminal failure, which
    /// ends the run through the ordinary UI-failure path.
    fn simulation_failed(&self) -> bool {
        let Some(message) = self.sim.as_ref().and_then(|sim| sim.slot.failure()) else {
            return false;
        };
        self.state.failure.borrow_mut().get_or_insert(message);
        true
    }

    /// The handle, if the UI VM was spawned.
    fn vm(&self) -> Option<UiVm> {
        self.slot.borrow().clone()
    }

    /// The text of the UI VM's terminal failure, if the link saw one.
    pub(crate) fn failure(&self) -> Option<String> {
        self.state.failure.borrow().clone()
    }

    /// The calls posted and not yet answered.
    pub(crate) fn outstanding(&self) -> usize {
        self.state.pending.borrow().len()
            + self.state.times.borrow().len()
            + self.state.hovers.borrow().len()
    }

    /// The names of the callbacks still running, oldest first.
    pub(crate) fn outstanding_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .state
            .pending
            .borrow()
            .iter()
            .map(|(_, event)| name_of(event))
            .collect();
        names.extend(
            self.state
                .times
                .borrow()
                .iter()
                .map(|_| crate::game_api::GAME_TIME),
        );
        names.extend(
            self.state
                .hovers
                .borrow()
                .iter()
                .map(|_| crate::game_api::GAME_HOVER),
        );
        names
    }

    /// Takes every result that is already there, never waiting, and returns every failure among them, in
    /// order (the post-stop fold: any remaining result is taken here). A failing `game_time` call is one of them.
    pub(crate) fn take_remaining_failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        let mut link = self.clone();
        // Results only: nothing new is posted after the stop.
        while let Some(step) = link.take_late() {
            failures.extend(step);
        }
        failures
    }
}

impl CodekarlUi {
    /// Posts the latest simulation snapshot as `game_time`, when a callback is registered, something displayed differs from
    /// the last snapshot delivered and there is a free call place outside the one reserved for lifecycle posts. Otherwise
    /// nothing is posted and the next pass tries again; an older snapshot is never posted once a newer one exists.
    fn deliver_time(&self) -> bool {
        let Some(sim) = &self.sim else {
            return false;
        };
        if !sim
            .time_registered
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            // A callback registered later gets the state at that time.
            self.state.delivered.set(None);
            return false;
        }
        let Some(snapshot) = sim.slot.latest() else {
            return false;
        };
        if self
            .state
            .delivered
            .get()
            .is_some_and(|delivered| !snapshot.differs_from(&delivered))
        {
            return false;
        }
        if self.outstanding() >= UI_CALL_LIMIT - 1 {
            return false;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        let posted = vm.borrow_mut().post(
            crate::game_api::GAME_TIME,
            vec![crate::game_api::time_table(&snapshot)],
        );
        if let Ok(id) = posted {
            self.state.times.borrow_mut().push(id);
            self.state.delivered.set(Some(snapshot));
            return true;
        }
        false
    }

    /// Posts only a changed latest hover, outside the lifecycle-reserved call place.
    fn deliver_hover(&self) -> bool {
        let (Some(map), Some(sim)) = (&self.map, &self.sim) else {
            return false;
        };
        if !sim
            .hover_registered
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            *self.state.delivered_hover.borrow_mut() = DeliveredHover::Unsent;
            return false;
        }
        let hover = crate::map_app::lock(map).hover();
        if matches!(
            &*self.state.delivered_hover.borrow(),
            DeliveredHover::Sent(delivered) if delivered == &hover
        ) || !self.state.hovers.borrow().is_empty()
            || self.outstanding() >= UI_CALL_LIMIT - 1
        {
            return false;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        if let Ok(id) = vm.borrow_mut().post(
            crate::game_api::GAME_HOVER,
            crate::game_api::hover_arguments(hover.as_ref()),
        ) {
            self.state.hovers.borrow_mut().push(id);
            *self.state.delivered_hover.borrow_mut() = DeliveredHover::Sent(hover);
            return true;
        }
        false
    }

    /// Gives both changed latest-wins channels a chance at each available place. When only one place
    /// is free, the channel that did not get the previous successful delivery goes first.
    fn deliver_updates(&self) {
        let first = self.state.next_update.get();
        let first_posted = match first {
            UpdateChannel::Time => self.deliver_time(),
            UpdateChannel::Hover => self.deliver_hover(),
        };
        let second = match first {
            UpdateChannel::Time => UpdateChannel::Hover,
            UpdateChannel::Hover => UpdateChannel::Time,
        };
        let second_posted = match second {
            UpdateChannel::Time => self.deliver_time(),
            UpdateChannel::Hover => self.deliver_hover(),
        };
        if first_posted || second_posted {
            self.state
                .next_update
                .set(if first_posted { second } else { first });
        }
    }

    /// The next result that is not a successful `game_time` call (those are taken here and dropped): an answer for the window loop,
    /// or the failure of a `game_time` call.
    fn take_next(&mut self) -> Option<Taken> {
        let vm = self.vm()?;
        loop {
            let result = vm.borrow_mut().take_result()?;
            let timed = {
                let mut times = self.state.times.borrow_mut();
                times
                    .iter()
                    .position(|id| *id == result.id)
                    .map(|position| times.remove(position))
            };
            if timed.is_some() {
                if let CallOutcome::Failed { message, traceback } = result.outcome {
                    return Some(Taken::TimeFailed(format!(
                        "ui callback {} failed: {message}\n{traceback}",
                        crate::game_api::GAME_TIME
                    )));
                }
                continue;
            }
            let hovered = {
                let mut hovers = self.state.hovers.borrow_mut();
                hovers
                    .iter()
                    .position(|id| *id == result.id)
                    .map(|position| hovers.remove(position))
            };
            if hovered.is_some() {
                if let CallOutcome::Failed { message, traceback } = result.outcome {
                    return Some(Taken::HoverFailed(format!(
                        "ui callback {} failed: {message}\n{traceback}",
                        crate::game_api::GAME_HOVER
                    )));
                }
                continue;
            }
            let mut pending = self.state.pending.borrow_mut();
            let position = pending.iter().position(|(id, _)| *id == result.id)?;
            let (_, event) = pending.remove(position)?;
            drop(pending);
            let answer = answer_of(&event, result.outcome);
            let answer = if self.map.is_some()
                && matches!(&event, UiEvent::Update(_))
                && matches!(answer, Answer::NotRegistered)
            {
                // The map controller consumes paced updates even when Lua has no frame_update callback. Returning a successful
                // synthetic answer keeps the scheduler's registration epoch alive without changing Lua's callback contract.
                Answer::Returned
            } else {
                answer
            };
            return Some(Taken::Answer(UiAnswer { event, answer }));
        }
    }

    /// The next answer for the window loop. A failing `game_time` call is the link's terminal failure here, which ends the run
    /// through the ordinary UI-failure path.
    fn next_answer(&mut self) -> Option<UiAnswer> {
        loop {
            match self.take_next()? {
                Taken::Answer(answer) => return Some(answer),
                Taken::TimeFailed(message) | Taken::HoverFailed(message) => {
                    self.state.failure.borrow_mut().get_or_insert(message);
                }
            }
        }
    }

    /// One step of the late paths (the drain after the loop and the post-stop fold), which never post anything new: `None` when no
    /// result is left, else `Some` of the failures the step took (none, or the one of an answer or of a `game_time` call).
    pub(crate) fn take_late(&mut self) -> Option<Vec<String>> {
        Some(match self.take_next()? {
            Taken::Answer(UiAnswer {
                answer: Answer::Failed { message },
                ..
            })
            | Taken::TimeFailed(message)
            | Taken::HoverFailed(message) => vec![message],
            Taken::Answer(_) => Vec::new(),
        })
    }
}

/// What the link took from the VM's results.
enum Taken {
    /// An answer to an event of the window loop.
    Answer(UiAnswer),
    /// A `game_time` call failed: the message with its traceback.
    TimeFailed(String),
    /// A `game_hover` call failed.
    HoverFailed(String),
}

impl UiLink for CodekarlUi {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        if self.simulation_failed() {
            return Err(PostError::Disconnected);
        }
        let Some(vm) = self.vm() else {
            return Err(PostError::Disconnected);
        };
        // The reserved slot (design §9): an update is refused, without a call to the VM, while the calls
        // outstanding are at least the limit less one, so there is always a slot left for `opened`, `close_request`
        // and the other lifecycle posts, which use the whole limit. Continuous updates cannot occupy every slot.
        if matches!(event, UiEvent::Update(_)) && self.outstanding() >= UI_CALL_LIMIT - 1 {
            return Err(PostError::Full);
        }
        let (callback, arguments) = match call_of(&event) {
            Ok(call) => call,
            Err(message) => {
                // Nothing was posted: the failure is the link's terminal one, as a VM failure is, and the loop
                // ends the run through the ordinary disconnected path.
                *self.state.failure.borrow_mut() = Some(message);
                return Err(PostError::Disconnected);
            }
        };
        let posted = vm.borrow_mut().post(callback, arguments);
        match posted {
            Ok(id) => {
                let map_result = match (&self.map, &event) {
                    (Some(map), UiEvent::Update(update)) => {
                        crate::map_app::lock(map).update(update).map(|_| ())
                    }
                    _ => Ok(()),
                };
                // Only after the UI VM accepted the post: a refused update leaves its transitions in the snapshot for the next
                // accepted one, so a press is acted on exactly once.
                if let (Some(sim), UiEvent::Update(update)) = (&self.sim, &event)
                    && sim.controls.send(&update.input)
                {
                    crate::lifecycle::emit(
                        ck_log::Level::Warn,
                        "sim",
                        format_args!("{}", crate::game::controls::OVERFLOW_RECORD),
                    );
                }
                self.state.pending.borrow_mut().push_back((id, event));
                if let Err(message) = map_result {
                    *self.state.failure.borrow_mut() = Some(message);
                    return Err(PostError::Disconnected);
                }
                Ok(())
            }
            Err(VmError::Full { .. }) => Err(PostError::Full),
            // `Disconnected`, and `State` (the handle is no longer Started): the VM has ended for us.
            Err(_) => {
                self.state.disconnected.set(true);
                Err(PostError::Disconnected)
            }
        }
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        if self.simulation_failed() {
            return None;
        }
        self.deliver_updates();
        let answer = self.next_answer();
        // Intake can drain hidden game_time/game_hover results before returning a frame answer. Those results may
        // free the non-lifecycle places even when `answer` is Some; re-arm waiting deliveries after every drain.
        self.deliver_updates();
        answer
    }

    fn is_disconnected(&self) -> bool {
        if self.simulation_failed()
            || self.state.failure.borrow().is_some()
            || self.state.disconnected.get()
        {
            return true;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        let mut vm = vm.borrow_mut();
        if let Some(error) = vm.take_failure() {
            *self.state.failure.borrow_mut() = Some(describe(error));
            return true;
        }
        matches!(vm.state(), VmState::Ending | VmState::Finished) || vm.is_finished()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use ck_lua::HostFunction;
    use ck_window::{ActionSnapshot, InputSnapshot, PixelSize, Transition, WindowMetrics};

    use super::*;
    use crate::testing::{Extras, Gate, Harness, WINDOW, frame_extension};

    fn returned(values: Vec<Option<ConfigValue>>) -> CallOutcome {
        CallOutcome::Returned(values)
    }

    const CLOSE: UiEvent = UiEvent::CloseRequested {
        source: CloseSource::Button,
    };

    #[test]
    fn a_close_request_result_is_accepted_only_by_a_first_value_of_true() {
        for (values, wanted) in [
            (vec![Some(ConfigValue::Bool(true))], Answer::Accept),
            (vec![Some(ConfigValue::Bool(false))], Answer::Decline),
            (vec![], Answer::Decline),
            (vec![None], Answer::Decline),
            (
                vec![Some(ConfigValue::String("true".to_owned()))],
                Answer::Decline,
            ),
            (vec![Some(ConfigValue::Integer(1))], Answer::Decline),
            (
                vec![
                    Some(ConfigValue::Bool(false)),
                    Some(ConfigValue::Bool(true)),
                ],
                Answer::Decline,
            ),
        ] {
            assert_eq!(
                answer_of(&CLOSE, returned(values.clone())),
                wanted,
                "{values:?}"
            );
        }
        assert_eq!(
            answer_of(&CLOSE, CallOutcome::NotRegistered),
            Answer::NotRegistered
        );
        // `opened` maps the same way (the loop ignores Accept and Decline there).
        assert_eq!(
            answer_of(
                &UiEvent::Opened,
                returned(vec![Some(ConfigValue::Bool(true))])
            ),
            Answer::Accept
        );
        assert_eq!(
            answer_of(&UiEvent::Opened, CallOutcome::NotRegistered),
            Answer::NotRegistered
        );
    }

    #[test]
    fn a_failed_callback_keeps_its_name_message_and_traceback() {
        let failed = |event: &UiEvent| {
            answer_of(
                event,
                CallOutcome::Failed {
                    message: "main.lua:3: boom".to_owned(),
                    traceback: "stack traceback:\n\t[C]: in ?".to_owned(),
                },
            )
        };
        assert_eq!(
            failed(&CLOSE),
            Answer::Failed {
                message: "ui callback close_request failed: main.lua:3: boom\nstack traceback:\n\t[C]: in ?"
                    .to_owned()
            }
        );
        assert_eq!(
            failed(&UiEvent::Opened),
            Answer::Failed {
                message:
                    "ui callback opened failed: main.lua:3: boom\nstack traceback:\n\t[C]: in ?"
                        .to_owned()
            }
        );
    }

    #[test]
    fn every_event_is_a_posted_call_with_its_arguments() {
        assert_eq!(call_of(&UiEvent::Opened), Ok(("opened", Vec::new())));
        for (source, text) in [(CloseSource::Button, "button"), (CloseSource::Key, "key")] {
            assert_eq!(
                call_of(&UiEvent::CloseRequested { source }),
                Ok(("close_request", vec![ConfigValue::String(text.to_owned())]))
            );
        }
    }

    /// The next answer, within a bound.
    fn answer(link: &mut CodekarlUi) -> UiAnswer {
        let give_up = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(answer) = link.take_answer() {
                return answer;
            }
            assert!(Instant::now() < give_up, "no answer in time");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_real_vm_accepts_declines_and_defaults_through_the_link() {
        let harness = Harness::new(
            "window.on_close_request(function(source) return source == 'key' end)\n\
             window.on_opened(function() error('opened failed') end)",
            Extras::default(),
        );
        let mut link = harness.ui.clone();
        let button = UiEvent::CloseRequested {
            source: CloseSource::Button,
        };
        let key = UiEvent::CloseRequested {
            source: CloseSource::Key,
        };
        for (event, wanted) in [(button, Answer::Decline), (key, Answer::Accept)] {
            link.post(event.clone()).expect("posted");
            let got = answer(&mut link);
            assert_eq!((got.event, got.answer), (event, wanted));
        }
        link.post(UiEvent::Opened).expect("posted");
        let got = answer(&mut link);
        assert_eq!(got.event, UiEvent::Opened);
        let Answer::Failed { message } = got.answer else {
            panic!("opened should have failed");
        };
        assert!(
            message.starts_with("ui callback opened failed: "),
            "{message}"
        );
        assert!(message.contains("opened failed"), "{message}");
        assert!(message.contains("stack traceback"), "{message}");
        assert_eq!(link.outstanding(), 0);
        assert!(
            !link.is_disconnected(),
            "a failed callback leaves the VM serving"
        );
    }

    #[test]
    fn no_callback_registered_is_the_default_answer() {
        let harness = Harness::new("", Extras::default());
        let mut link = harness.ui.clone();
        link.post(CLOSE).expect("posted");
        assert_eq!(answer(&mut link).answer, Answer::NotRegistered);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the callback delivery sequence is one end-to-end behavior"
    )]
    fn hover_delivery_is_change_only_tracks_camera_and_publishes_nil_on_leave() {
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                let value = match arguments.first().and_then(Option::as_ref) {
                    Some(ConfigValue::Table(fields)) => match fields.get("name") {
                        Some(ConfigValue::String(name)) => name.clone(),
                        _ => "missing-name".to_owned(),
                    },
                    _ => "nil".to_owned(),
                };
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(value);
                Ok(Vec::new())
            })
        };
        let registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let harness = Harness::new(
            "game.on_hover(function(h) window.record(h) end)\nframe.on_update(function() end)",
            Extras {
                functions: vec![("record", record)],
                extensions: vec![
                    frame_extension(),
                    game_api::extension(
                        Arc::clone(&registered),
                        Arc::clone(&hover_registered),
                        Arc::new(|| {}),
                    ),
                ],
                ..Extras::default()
            },
        );
        let map = crate::map_app::tests::window_case_hover_controller();
        // The Cocoa helper injects at the fixture marker's center in window coordinates.
        // The window adapter reports top-left coordinates to the map controller.
        let pointer = ck_window::PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let map = crate::map_app::shared(map);
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), map).with_sim(SimLink {
            controls: crate::game::controls::SimControls::new(sender),
            slot,
            time_registered: registered,
            hover_registered,
        });
        let wait_for_count = |link: &mut CodekarlUi, wanted: usize| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let _ = link.take_answer();
                if seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len()
                    >= wanted
                {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "hover callback {wanted} was not delivered"
                );
                thread::sleep(Duration::from_millis(1));
            }
        };
        let mut first = update(1);
        let UiEvent::Update(first) = &mut first else {
            unreachable!();
        };
        first.metrics.points = (400, 240);
        first.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        first.input.pointer_position = Some(pointer);
        first.input.pointer_events = vec![
            ck_window::PointerEvent::Scroll {
                position: pointer,
                amount: 15 * 256,
                unit: ck_window::ScrollUnit::Points,
            },
            ck_window::PointerEvent::Move(pointer),
        ];
        first.input.actions = vec![action("map_pan_left", false, true, true)];
        first.input.transitions = vec![
            Transition {
                action: "map_pan_left".to_owned(),
                edge: Edge::Press,
            },
            Transition {
                action: "map_pan_left".to_owned(),
                edge: Edge::Release,
            },
        ];
        link.post(UiEvent::Update(first.clone()))
            .expect("first update posted");

        // Model more dropped owner input arriving after the pointer move, before the link gets a
        // chance to publish the controller's hover to the UI VM.
        let mut interleaved_keys = update(2);
        let UiEvent::Update(interleaved_keys) = &mut interleaved_keys else {
            unreachable!();
        };
        interleaved_keys.metrics.points = (400, 240);
        interleaved_keys.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        interleaved_keys.input.pointer_position = Some(pointer);
        interleaved_keys.input.actions = vec![action("map_zoom_out", false, true, true)];
        interleaved_keys.input.transitions = vec![
            Transition {
                action: "map_zoom_out".to_owned(),
                edge: Edge::Press,
            },
            Transition {
                action: "map_zoom_out".to_owned(),
                edge: Edge::Release,
            },
        ];
        link.post(UiEvent::Update(interleaved_keys.clone()))
            .expect("interleaved key update posted");

        let _ = answer(&mut link);
        wait_for_count(&mut link, 1);
        assert_eq!(seen.lock().unwrap().as_slice(), ["Fixture Town"]);

        let mut still = update(3);
        let UiEvent::Update(still) = &mut still else {
            unreachable!();
        };
        still.metrics.points = (400, 240);
        still.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        still.input.pointer_position = Some(pointer);
        link.post(UiEvent::Update(still.clone()))
            .expect("still update posted");
        let _ = answer(&mut link);
        for _ in 0..5 {
            let _ = link.take_answer();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(seen.lock().unwrap().as_slice(), ["Fixture Town"]);

        let mut pan = update(4);
        let UiEvent::Update(pan) = &mut pan else {
            unreachable!();
        };
        pan.metrics.points = (400, 240);
        pan.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        pan.input.actions = vec![action("map_pan_right", true, false, false)];
        pan.input.pointer_position = Some(pointer);
        link.post(UiEvent::Update(pan.clone()))
            .expect("pan update posted");
        let _ = answer(&mut link);
        wait_for_count(&mut link, 2);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["Fixture Town", "Fixture Town"]
        );

        let mut leave = update(5);
        let UiEvent::Update(leave) = &mut leave else {
            unreachable!();
        };
        leave.metrics.points = (400, 240);
        leave.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        leave.input.pointer_position = Some(ck_window::PointerPosition {
            x: 401 * 256,
            y: 12 * 256,
        });
        link.post(UiEvent::Update(leave.clone()))
            .expect("leave update posted");
        let _ = answer(&mut link);
        wait_for_count(&mut link, 3);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["Fixture Town", "Fixture Town", "nil"]
        );
    }

    #[test]
    fn posting_beyond_the_call_limit_is_full_and_nothing_is_sent() {
        let gate = Gate::new();
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            Arc::new(move |_| {
                gate.wait();
                Ok(Vec::new())
            })
        };
        let harness = Harness::new(
            &format!("{WINDOW}\nwindow.on_opened(function() window.hold() end)"),
            Extras {
                functions: vec![("hold", hold)],
                after_wake: None,
                ..Extras::default()
            },
        );
        let mut link = harness.ui.clone();
        // The limit is 4 calls in flight (a call counts until its result is taken).
        for _ in 0..4 {
            link.post(UiEvent::Opened).expect("within the limit");
        }
        assert_eq!(link.post(UiEvent::Opened), Err(PostError::Full));
        assert_eq!(link.outstanding(), 4, "the refused call was not sent");
        gate.open();
        for _ in 0..4 {
            let _ = answer(&mut link);
        }
        link.post(UiEvent::Opened).expect("room again");
        let _ = answer(&mut link);
    }

    // --- the paced update (`frame_update`)

    fn action(name: &str, held: bool, pressed: bool, released: bool) -> ActionSnapshot {
        ActionSnapshot {
            name: name.to_owned(),
            held,
            pressed,
            released,
        }
    }

    /// An update with a known snapshot: `move_left` held, `reload_texture` tapped, `move_up` unbound and untouched.
    fn update(sequence: u64) -> UiEvent {
        UiEvent::Update(UpdateEvent {
            sequence,
            elapsed: Duration::from_millis(500),
            dt: Duration::from_millis(250),
            metrics: WindowMetrics {
                points: (100, 80),
                pixels: PixelSize {
                    width: 200,
                    height: 160,
                },
            },
            input: InputSnapshot {
                sequence: 7,
                actions: vec![
                    action("move_left", true, true, false),
                    action("move_up", false, false, false),
                    action("reload_texture", false, true, true),
                ],
                transitions: vec![
                    Transition {
                        action: "move_left".to_owned(),
                        edge: Edge::Press,
                    },
                    Transition {
                        action: "reload_texture".to_owned(),
                        edge: Edge::Press,
                    },
                    Transition {
                        action: "reload_texture".to_owned(),
                        edge: Edge::Release,
                    },
                ],
                overflow: true,
                pointer_events: Vec::new(),
                pointer_position: None,
                pointer_held: false,
                pointer_overflow: false,
                reset: false,
                overflow_count: 3,
            },
        })
    }

    fn flags(held: bool, pressed: bool, released: bool) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            ("held".to_owned(), ConfigValue::Bool(held)),
            ("pressed".to_owned(), ConfigValue::Bool(pressed)),
            ("released".to_owned(), ConfigValue::Bool(released)),
        ]))
    }

    fn edge(action: &str, kind: &str) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            ("action".to_owned(), ConfigValue::String(action.to_owned())),
            ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
        ]))
    }

    /// The table of `update(sequence)`, written out (design §8.2).
    fn expected_table(sequence: i64) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            ("dt".to_owned(), ConfigValue::Float(0.25)),
            ("elapsed".to_owned(), ConfigValue::Float(0.5)),
            ("sequence".to_owned(), ConfigValue::Integer(sequence)),
            ("width".to_owned(), ConfigValue::Integer(100)),
            ("height".to_owned(), ConfigValue::Integer(80)),
            ("pixel_width".to_owned(), ConfigValue::Integer(200)),
            ("pixel_height".to_owned(), ConfigValue::Integer(160)),
            (
                "actions".to_owned(),
                ConfigValue::Table(BTreeMap::from([
                    ("move_left".to_owned(), flags(true, true, false)),
                    ("move_up".to_owned(), flags(false, false, false)),
                    ("reload_texture".to_owned(), flags(false, true, true)),
                ])),
            ),
            (
                "transitions".to_owned(),
                ConfigValue::Array(vec![
                    edge("move_left", "press"),
                    edge("reload_texture", "press"),
                    edge("reload_texture", "release"),
                ]),
            ),
            ("overflow".to_owned(), ConfigValue::Bool(true)),
            ("reset".to_owned(), ConfigValue::Bool(false)),
        ]))
    }

    #[test]
    fn an_update_is_the_frame_update_call_with_the_table_of_the_design() {
        // Negative control: a wrong name, a missing key, an unbound action left out or a transition's kind swapped
        // changes this exact value.
        assert_eq!(
            call_of(&update(5)),
            Ok(("frame_update", vec![expected_table(5)]))
        );
        assert_eq!(name_of(&update(5)), FRAME_UPDATE);
    }

    #[test]
    fn the_sequence_is_converted_exactly_up_to_i64_max_and_refused_beyond() {
        // Negative control: saturating (`unwrap_or(i64::MAX)`) or wrapping the conversion makes the first assertion
        // below fail (a callback would be given i64::MAX for 2^63) or the failure text absent.
        let top = u64::try_from(i64::MAX).expect("positive");
        assert_eq!(
            call_of(&update(top)),
            Ok(("frame_update", vec![expected_table(i64::MAX)]))
        );
        let message = call_of(&update(top + 1)).expect_err("2^63 is not a Lua integer");
        assert!(message.contains(&(top + 1).to_string()), "{message}");
        assert!(message.contains("Lua integer"), "{message}");
        assert!(call_of(&update(u64::MAX)).is_err());
    }

    #[test]
    fn an_unrepresentable_sequence_ends_the_link_with_a_failure_and_posts_no_callback() {
        let seen = Arc::new(std::sync::Mutex::new(0_u32));
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |_| {
                *seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
                Ok(Vec::new())
            })
        };
        let harness = frame_harness("window.record(f)", vec![("record", record)]);
        let mut link = harness.ui.clone();
        let top = u64::try_from(i64::MAX).expect("positive");
        link.post(update(top))
            .expect("the largest sequence is posted");
        assert_eq!(answer(&mut link).answer, Answer::Returned);
        assert_eq!(link.post(update(top + 1)), Err(PostError::Disconnected));
        assert_eq!(link.outstanding(), 0, "nothing was sent to the VM");
        let failure = link.failure().expect("the link failed");
        assert!(failure.contains(&(top + 1).to_string()), "{failure}");
        assert!(link.is_disconnected());
        assert_eq!(
            *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            1,
            "only the representable update reached a callback"
        );
    }

    #[test]
    fn an_update_with_no_actions_and_no_transitions_has_empty_collections() {
        let UiEvent::Update(mut event) = update(1) else {
            unreachable!("update() is an update");
        };
        event.input.actions.clear();
        event.input.transitions.clear();
        let (_, arguments) = call_of(&UiEvent::Update(event)).expect("a call");
        let ConfigValue::Table(table) = &arguments[0] else {
            panic!("a table: {arguments:?}");
        };
        assert_eq!(table["actions"], ConfigValue::Table(BTreeMap::new()));
        assert_eq!(table["transitions"], ConfigValue::Array(Vec::new()));
    }

    #[test]
    fn an_update_answer_ignores_the_values_and_maps_the_rest_like_the_other_callbacks() {
        let event = update(1);
        for values in [
            vec![],
            vec![Some(ConfigValue::Bool(true))],
            vec![Some(ConfigValue::Bool(false))],
            vec![Some(ConfigValue::Integer(3))],
        ] {
            assert_eq!(
                answer_of(&event, returned(values.clone())),
                Answer::Returned,
                "{values:?}: any return completes the update"
            );
        }
        assert_eq!(
            answer_of(&event, CallOutcome::NotRegistered),
            Answer::NotRegistered
        );
        assert_eq!(
            answer_of(
                &event,
                CallOutcome::Failed {
                    message: "main.lua:3: boom".to_owned(),
                    traceback: "stack traceback:\n\t[C]: in ?".to_owned(),
                }
            ),
            Answer::Failed {
                message: "ui callback frame_update failed: main.lua:3: boom\nstack traceback:\n\t[C]: in ?"
                    .to_owned()
            }
        );
    }

    /// A harness whose script registers a `frame_update` callback running `body` on the table `f`.
    fn frame_harness(body: &str, functions: Vec<(&'static str, HostFunction)>) -> Harness {
        Harness::new(
            &format!("frame.on_update(function(f)\n{body}\nend)"),
            Extras {
                functions,
                extensions: vec![frame_extension()],
                ..Extras::default()
            },
        )
    }

    #[test]
    fn a_real_vm_gets_the_table_a_script_reads_and_a_returning_callback_completes_the_update() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                if let Some(Some(value)) = arguments.first() {
                    seen.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(value.clone());
                }
                Ok(Vec::new())
            })
        };
        let harness = frame_harness(
            "window.record(f)\nreturn 'ignored'",
            vec![("record", record)],
        );
        let mut link = harness.ui.clone();
        link.post(update(9)).expect("posted");
        let got = answer(&mut link);
        assert_eq!(got.answer, Answer::Returned);
        assert_eq!(got.event, update(9));
        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            &*seen,
            &[expected_table(9)],
            "the script read exactly the table of the design (a held and a tapped action, an unbound action present and false, the transitions in order, overflow and reset)"
        );
        assert_eq!(link.outstanding(), 0);
    }

    #[test]
    fn an_unregistered_update_is_not_registered_and_a_raising_one_fails_with_its_traceback() {
        let harness = Harness::new("", Extras::default());
        let mut link = harness.ui.clone();
        link.post(update(1)).expect("posted");
        assert_eq!(answer(&mut link).answer, Answer::NotRegistered);

        let harness = frame_harness("error('update boom')", Vec::new());
        let mut link = harness.ui.clone();
        link.post(update(2)).expect("posted");
        let got = answer(&mut link);
        let Answer::Failed { message } = got.answer else {
            panic!("the callback raised: {:?}", got.answer);
        };
        assert!(
            message.starts_with("ui callback frame_update failed: "),
            "{message}"
        );
        assert!(message.contains("update boom"), "{message}");
        assert!(message.contains("stack traceback"), "{message}");
        assert!(
            !link.is_disconnected(),
            "a raising callback leaves the VM serving"
        );
    }

    /// A harness whose registered callbacks all hold on `gate`, so the calls stay outstanding.
    fn held_harness(gate: &Arc<Gate>) -> Harness {
        let hold: HostFunction = {
            let gate = Arc::clone(gate);
            Arc::new(move |_| {
                gate.wait();
                Ok(Vec::new())
            })
        };
        Harness::new(
            &format!(
                "{WINDOW}\nframe.on_update(function() window.hold() end)\n\
                 window.on_close_request(function() window.hold() return true end)\n\
                 window.on_opened(function() window.hold() end)"
            ),
            Extras {
                functions: vec![("hold", hold)],
                extensions: vec![frame_extension()],
                ..Extras::default()
            },
        )
    }

    #[test]
    fn updates_leave_a_slot_for_close_and_lifecycle() {
        // Negative control: without the reservation, filling the slots with updates leaves no room, and the close
        // post below is `Full`.
        let gate = Gate::new();
        let harness = held_harness(&gate);
        let mut link = harness.ui.clone();
        for sequence in 1..=3 {
            link.post(update(sequence))
                .expect("within the reserved limit");
        }
        assert_eq!(
            link.post(update(4)),
            Err(PostError::Full),
            "limit - 1 calls outstanding: an update is refused"
        );
        assert_eq!(
            link.outstanding(),
            UI_CALL_LIMIT - 1,
            "the refused update was not sent to the VM"
        );
        // A close request, and `opened`, use the whole limit.
        link.post(CLOSE)
            .expect("the reserved slot is for the close request");
        assert_eq!(link.outstanding(), UI_CALL_LIMIT);
        assert_eq!(
            link.post(CLOSE),
            Err(PostError::Full),
            "the VM's limit itself"
        );
        gate.open();
        for _ in 0..UI_CALL_LIMIT {
            let _ = answer(&mut link);
        }
        // With fewer outstanding the update is posted, and the limit is the VM's again.
        link.post(update(5)).expect("room again");
        link.post(UiEvent::Opened)
            .expect("opened uses the whole limit");
        link.post(CLOSE).expect("and so does a close");
        link.post(UiEvent::Opened).expect("and the last slot");
        assert_eq!(link.post(update(6)), Err(PostError::Full));
        for _ in 0..UI_CALL_LIMIT {
            let _ = answer(&mut link);
        }
    }

    #[test]
    fn an_update_is_refused_without_calling_the_vm_while_the_reserved_slot_is_all_that_is_left() {
        let gate = Gate::new();
        let harness = held_harness(&gate);
        let mut link = harness.ui.clone();
        for _ in 0..UI_CALL_LIMIT - 1 {
            link.post(UiEvent::Opened).expect("within the limit");
        }
        // The VM would take one more call, but not an update: the refusal is the link's own.
        assert_eq!(link.post(update(1)), Err(PostError::Full));
        assert_eq!(link.outstanding(), UI_CALL_LIMIT - 1);
        link.post(CLOSE).expect("the close request still posts");
        gate.open();
        for _ in 0..UI_CALL_LIMIT {
            let _ = answer(&mut link);
        }
    }

    #[test]
    fn the_link_is_disconnected_once_the_handle_has_ended_for_the_caller() {
        // No UI VM at all: not disconnected (nothing to lose), but nothing can be posted.
        let mut empty = CodekarlUi::new(UiSlot::default());
        assert!(!empty.is_disconnected());
        assert_eq!(empty.post(UiEvent::Opened), Err(PostError::Disconnected));
        assert!(empty.take_answer().is_none());

        // A cancelled handle is Ending: disconnected, and a post says so.
        let harness = Harness::new("", Extras::default());
        let mut link = harness.ui.clone();
        assert!(!link.is_disconnected());
        harness.vm.borrow_mut().cancel();
        assert!(link.is_disconnected());
        assert_eq!(link.post(UiEvent::Opened), Err(PostError::Disconnected));
    }

    #[test]
    fn a_terminal_failure_is_remembered_with_its_text() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let boom: HostFunction = Arc::new(move |_| {
            flag.store(true, Ordering::SeqCst);
            panic!("a host panic in a callback");
        });
        let harness = Harness::new(
            "window.on_opened(function() window.boom() end)",
            Extras {
                functions: vec![("boom", boom)],
                after_wake: None,
                ..Extras::default()
            },
        );
        let mut link = harness.ui.clone();
        link.post(UiEvent::Opened).expect("posted");
        let give_up = Instant::now() + Duration::from_secs(10);
        while !link.is_disconnected() {
            assert!(Instant::now() < give_up, "the failure was never seen");
            thread::sleep(Duration::from_millis(1));
        }
        assert!(ran.load(Ordering::SeqCst));
        let text = link.failure().expect("the failure text");
        assert!(text.contains("a host panic in a callback"), "{text}");
        assert!(link.is_disconnected(), "and it stays disconnected");
        assert!(
            link.take_answer().is_none(),
            "a host panic sends no call result"
        );
    }

    // --- the simulation's time (`game_time`)

    use crate::game::slot::{PauseReason, SimSlot, SimSnapshot};
    use crate::game_api;

    fn snapshot_at(hour: u8, speed: u8, paused: Option<PauseReason>) -> SimSnapshot {
        let date = ck_sim::Date {
            year: 768,
            month: 10,
            day: 9,
            hour,
        };
        SimSnapshot {
            tick: ck_sim::GameTime::from_date(date).expect("a valid date"),
            date,
            speed: ck_sim::Speed::new(speed).expect("a speed"),
            paused,
        }
    }

    /// A UI VM running `script` with `game.on_time` and a `window.record` that notes `hour speed paused`, over a link with a
    /// simulation slot the test publishes to.
    struct Timed {
        harness: Harness,
        link: CodekarlUi,
        slot: Arc<SimSlot>,
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    fn timed(script: &str, extras: Extras) -> Timed {
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                let text: Vec<String> = arguments
                    .iter()
                    .flatten()
                    .map(|value| match value {
                        ConfigValue::Integer(number) => number.to_string(),
                        ConfigValue::String(text) => text.clone(),
                        ConfigValue::Bool(flag) => flag.to_string(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(text.join(" "));
                Ok(Vec::new())
            })
        };
        let registered = Arc::new(AtomicBool::new(false));
        let mut extras = extras;
        extras.functions.push(("record", record));
        extras.extensions.push(game_api::extension(
            Arc::clone(&registered),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|| {}),
        ));
        let harness = Harness::new(script, extras);
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let link = harness.ui.clone().with_sim(SimLink {
            controls: crate::game::controls::SimControls::new(sender),
            slot: Arc::clone(&slot),
            time_registered: registered,
            hover_registered: Arc::new(AtomicBool::new(false)),
        });
        Timed {
            harness,
            link,
            slot,
            seen,
        }
    }

    impl Timed {
        fn seen(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// Runs the link's passes until `count` `game_time` calls have run, at most ten seconds.
        fn pump_until(&mut self, count: usize) {
            let give_up = Instant::now() + Duration::from_secs(10);
            while self.seen().len() < count {
                let _ = self.link.take_answer();
                assert!(
                    Instant::now() < give_up,
                    "{} calls, not {count}",
                    self.seen().len()
                );
                thread::yield_now();
            }
            // The last call's result is taken too.
            while self.link.outstanding() > 0 {
                let _ = self.link.take_answer();
                assert!(Instant::now() < give_up, "a call is still outstanding");
                thread::yield_now();
            }
        }
    }

    const ON_TIME: &str =
        "game.on_time(function(t) window.record(t.hour, t.speed, tostring(t.paused)) end)";

    #[test]
    fn a_changed_snapshot_is_delivered_once_and_an_unchanged_one_not_at_all() {
        let mut timed = timed(ON_TIME, Extras::default());
        timed
            .slot
            .publish(snapshot_at(14, 1, Some(PauseReason::Load)));
        timed.pump_until(1);
        assert_eq!(timed.seen(), ["14 1 load"]);
        // The same displayed state, published again: nothing is posted.
        timed
            .slot
            .publish(snapshot_at(14, 1, Some(PauseReason::Load)));
        for _ in 0..5 {
            let _ = timed.link.take_answer();
        }
        assert_eq!(timed.link.outstanding(), 0);
        assert_eq!(timed.seen().len(), 1);
        // A tick that changes the hour, a speed picked while paused, and a pause for another reason each deliver once.
        timed
            .slot
            .publish(snapshot_at(15, 1, Some(PauseReason::Load)));
        timed.pump_until(2);
        timed
            .slot
            .publish(snapshot_at(15, 3, Some(PauseReason::Load)));
        timed.pump_until(3);
        timed.slot.publish(snapshot_at(15, 3, None));
        timed.pump_until(4);
        assert_eq!(
            timed.seen(),
            ["14 1 load", "15 1 load", "15 3 load", "15 3 false"]
        );
    }

    #[test]
    fn with_the_call_places_full_the_delivery_is_retried_and_only_the_latest_is_posted() {
        let gate = Gate::new();
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            Arc::new(move |_| {
                gate.wait();
                Ok(Vec::new())
            })
        };
        let script = format!("{ON_TIME}\nwindow.on_opened(function() window.hold() end)");
        let mut timed = timed(
            &script,
            Extras {
                functions: vec![("hold", hold)],
                ..Extras::default()
            },
        );
        // Three held calls leave the one place reserved for lifecycle posts: nothing may be delivered.
        for _ in 0..3 {
            timed.link.post(UiEvent::Opened).expect("within the limit");
        }
        timed
            .slot
            .publish(snapshot_at(1, 1, Some(PauseReason::Load)));
        let _ = timed.link.take_answer();
        timed.slot.publish(snapshot_at(2, 5, None));
        let _ = timed.link.take_answer();
        assert!(timed.seen().is_empty());
        assert_eq!(timed.link.outstanding(), 3);
        // The places free up: the retry posts the latest snapshot, and the older one is never posted.
        gate.open();
        timed.pump_until(1);
        assert_eq!(timed.seen(), ["2 5 false"]);
        for _ in 0..5 {
            let _ = timed.link.take_answer();
        }
        assert_eq!(timed.seen().len(), 1);
    }

    #[test]
    fn without_a_registered_callback_nothing_is_posted() {
        let mut timed = timed("", Extras::default());
        timed.slot.publish(snapshot_at(9, 2, None));
        for _ in 0..5 {
            let _ = timed.link.take_answer();
        }
        assert_eq!(timed.link.outstanding(), 0);
        assert!(timed.seen().is_empty());
    }

    #[test]
    fn a_failing_game_time_callback_is_the_links_terminal_failure() {
        let mut timed = timed(
            "game.on_time(function() error('readout exploded') end)",
            Extras::default(),
        );
        timed.slot.publish(snapshot_at(9, 2, None));
        let give_up = Instant::now() + Duration::from_secs(10);
        while !timed.link.is_disconnected() {
            let _ = timed.link.take_answer();
            assert!(Instant::now() < give_up, "the failure did not arrive");
            thread::yield_now();
        }
        let failure = timed.link.failure().expect("a failure");
        assert!(
            failure.starts_with("ui callback game_time failed: "),
            "{failure}"
        );
        assert!(failure.contains("readout exploded"), "{failure}");
        drop(timed.harness);
    }

    #[test]
    fn delivery_is_re_armed_by_the_pass_that_drains_the_results_which_freed_the_places() {
        let gate = Gate::new();
        // Set by the first held call on the owner thread, before it waits at the gate.
        let entered = Arc::new(AtomicBool::new(false));
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            let entered = Arc::clone(&entered);
            Arc::new(move |_| {
                entered.store(true, Ordering::SeqCst);
                gate.wait();
                Ok(Vec::new())
            })
        };
        let script = "game.on_time(function(t) window.hold() window.record(t.hour) end)";
        // Each report the VM sends (a call's result) is counted: the completion witness of a call, which `window.record` is not,
        // since it runs before the callback returns.
        let reports = Arc::new(AtomicUsize::new(0));
        let mut timed = timed(
            script,
            Extras {
                functions: vec![("hold", hold)],
                after_wake: Some({
                    let reports = Arc::clone(&reports);
                    Arc::new(move || {
                        reports.fetch_add(1, Ordering::SeqCst);
                    })
                }),
                ..Extras::default()
            },
        );
        // Three held `game_time` calls fill the places outside the reserved one; the fourth snapshot waits.
        for hour in 1..=3 {
            timed.slot.publish(snapshot_at(hour, 1, None));
            let _ = timed.link.take_answer();
        }
        assert_eq!(timed.link.outstanding(), 3);
        timed.slot.publish(snapshot_at(4, 1, None));
        let _ = timed.link.take_answer();
        assert_eq!(timed.link.outstanding(), 3, "no place for the fourth");
        // The baseline of the report count is read only once the first call is held at the gate. The owner thread sent its
        // start-up reports and ran their wake hooks before it took that call, and it is blocked in it now, so the count cannot
        // move until the gate opens: no late start-up wake can be counted as a call's report.
        let give_up = Instant::now() + Duration::from_secs(10);
        while !entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < give_up, "the first call was not held");
            thread::yield_now();
        }
        let before = reports.load(Ordering::SeqCst);
        // The calls finish (the test waits for their reports without touching the link), and then ONE pass is all there is: it
        // drains the three results, and that pass alone must post the waiting snapshot.
        gate.open();
        let give_up = Instant::now() + Duration::from_secs(10);
        while reports.load(Ordering::SeqCst) < before + 3 {
            assert!(Instant::now() < give_up, "the calls did not finish");
            thread::yield_now();
        }
        assert_eq!(timed.seen(), ["1", "2", "3"]);
        assert!(timed.link.take_answer().is_none());
        assert_eq!(
            timed.link.outstanding(),
            1,
            "the waiting snapshot was posted by the draining pass"
        );
        timed.pump_until(4);
        assert_eq!(timed.seen(), ["1", "2", "3", "4"]);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one end-to-end regression proves a busy call drain rearms the changed hover"
    )]
    fn a_hover_changed_with_full_places_is_rearmed_when_the_drain_also_returns_a_frame_answer() {
        let gate = Gate::new();
        let entered = Arc::new(AtomicBool::new(false));
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            let entered = Arc::clone(&entered);
            Arc::new(move |_| {
                entered.store(true, Ordering::SeqCst);
                gate.wait();
                Ok(Vec::new())
            })
        };
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                let value = match arguments.first().and_then(Option::as_ref) {
                    Some(ConfigValue::Table(fields)) => match fields.get("name") {
                        Some(ConfigValue::String(name)) => name.clone(),
                        _ => "missing-name".to_owned(),
                    },
                    _ => "nil".to_owned(),
                };
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(value);
                Ok(Vec::new())
            })
        };
        let time_registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let reports = Arc::new(AtomicUsize::new(0));
        let harness = Harness::new(
            "game.on_time(function() window.hold() end)\n\
             game.on_hover(function(h) window.record(h) end)\n\
             frame.on_update(function() end)",
            Extras {
                functions: vec![("hold", hold), ("record", record)],
                extensions: vec![
                    frame_extension(),
                    game_api::extension(
                        Arc::clone(&time_registered),
                        Arc::clone(&hover_registered),
                        Arc::new(|| {}),
                    ),
                ],
                after_wake: Some({
                    let reports = Arc::clone(&reports);
                    Arc::new(move || {
                        reports.fetch_add(1, Ordering::SeqCst);
                    })
                }),
                ..Extras::default()
            },
        );
        let map = crate::map_app::shared(crate::map_app::tests::window_case_hover_controller());
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut link =
            CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map)).with_sim(SimLink {
                controls: crate::game::controls::SimControls::new(sender),
                slot: Arc::clone(&slot),
                time_registered,
                hover_registered,
            });

        // Establish the unchanged nil delivery so the later marker hover is a distinct latest value.
        link.deliver_hover();
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            let _ = link.take_answer();
            assert!(
                Instant::now() < deadline,
                "the baseline nil hover was not delivered"
            );
            thread::yield_now();
        }
        while link.outstanding() > 0 {
            let _ = link.take_answer();
            assert!(
                Instant::now() < deadline,
                "the baseline hover result was not drained"
            );
            thread::yield_now();
        }
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        // Put the frame answer first in the result queue, then use the remaining two places for
        // held game_time calls. Change the controller hover only after those places are full, so
        // time and hover will both be waiting for the one place freed by the frame answer.
        link.post(update(1)).expect("frame update is posted first");
        slot.publish(snapshot_at(1, 1, None));
        link.deliver_updates();
        slot.publish(snapshot_at(2, 1, None));
        link.deliver_updates();
        assert_eq!(link.outstanding(), 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !entered.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "game_time did not enter the hold"
            );
            thread::yield_now();
        }
        let result_baseline = reports.load(Ordering::SeqCst);

        let pointer = ck_window::PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let mut event = update(1);
        let UiEvent::Update(update) = &mut event else {
            unreachable!();
        };
        update.metrics.points = (400, 240);
        update.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        update.input.pointer_position = Some(pointer);
        update.input.pointer_events = vec![
            ck_window::PointerEvent::Scroll {
                position: pointer,
                amount: 15 * 256,
                unit: ck_window::ScrollUnit::Points,
            },
            ck_window::PointerEvent::Move(pointer),
        ];
        let UiEvent::Update(controller_update) = &event else {
            unreachable!();
        };
        crate::map_app::lock(&map)
            .update(controller_update)
            .expect("the direct controller replay succeeds");
        assert_eq!(
            crate::map_app::lock(&map)
                .hover()
                .as_ref()
                .map(|hover| hover.name.as_str()),
            Some("Fixture Town")
        );
        slot.publish(snapshot_at(3, 1, None));
        assert_eq!(link.outstanding(), 3);

        // No link polling while the owner completes the frame and blocks in game_time.
        gate.open();
        let deadline = Instant::now() + Duration::from_secs(5);
        while reports.load(Ordering::SeqCst) < result_baseline + 2 {
            assert!(
                Instant::now() < deadline,
                "the held game_time calls did not finish"
            );
            thread::yield_now();
        }

        // This intake returns the first frame answer while both time calls remain outstanding.
        // The newer time snapshot and changed hover now compete for exactly one freed place.
        let answer = link
            .take_answer()
            .expect("the first frame update answer is ready");
        assert!(matches!(answer.event, UiEvent::Update(_)));
        let rearmed = link.outstanding_names();
        gate.open();
        assert_eq!(
            rearmed,
            [
                game_api::GAME_TIME,
                game_api::GAME_TIME,
                game_api::GAME_HOVER,
            ],
            "a steady stream of fresh time snapshots must not take the only place ahead of hover"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            assert!(
                Instant::now() < deadline,
                "the waiting hover was stranded after intake"
            );
            thread::yield_now();
        }
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            ["Fixture Town"]
        );
    }

    #[test]
    fn a_stream_of_hover_changes_cannot_starve_a_waiting_game_time_delivery() {
        let time_registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let harness = Harness::new(
            "game.on_time(function() end)\n\
             game.on_hover(function() end)\n\
             frame.on_update(function() end)",
            Extras {
                extensions: vec![
                    frame_extension(),
                    game_api::extension(
                        Arc::clone(&time_registered),
                        Arc::clone(&hover_registered),
                        Arc::new(|| {}),
                    ),
                ],
                ..Extras::default()
            },
        );
        let map = crate::map_app::shared(crate::map_app::tests::window_case_hover_controller());
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut link =
            CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map)).with_sim(SimLink {
                controls: crate::game::controls::SimControls::new(sender),
                slot: Arc::clone(&slot),
                time_registered,
                hover_registered,
            });

        // The first frame is the ready answer at the head of the VM queue. A marker hover is
        // delivered after it, making time the next fair choice when that frame frees one place.
        link.post(update(1)).expect("first frame is posted");
        let pointer = ck_window::PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let mut marker_event = update(2);
        let UiEvent::Update(marker) = &mut marker_event else {
            unreachable!();
        };
        marker.metrics.points = (400, 240);
        marker.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        marker.input.pointer_position = Some(pointer);
        marker.input.pointer_events = vec![ck_window::PointerEvent::Move(pointer)];
        crate::map_app::lock(&map)
            .update(marker)
            .expect("the marker hover is produced");
        link.deliver_updates();
        assert_eq!(link.outstanding_names(), ["frame_update", "game_hover"]);

        link.post(update(3))
            .expect("second frame fills the last place");
        slot.publish(snapshot_at(1, 1, None));

        // Keep changing the latest hover while all places are occupied. The already-posted hover
        // remains in flight, so the latest hover cannot be sent until that result is drained.
        let mut latest = update(4);
        let UiEvent::Update(latest) = &mut latest else {
            unreachable!();
        };
        latest.metrics.points = (400, 240);
        latest.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        for position in [
            ck_window::PointerPosition {
                x: 700 * 256,
                y: 400 * 256,
            },
            pointer,
            ck_window::PointerPosition {
                x: 700 * 256,
                y: 400 * 256,
            },
        ] {
            latest.input.pointer_position = Some(position);
            latest.input.pointer_events = vec![ck_window::PointerEvent::Move(position)];
            crate::map_app::lock(&map)
                .update(latest)
                .expect("the latest hover change is valid");
        }
        assert!(crate::map_app::lock(&map).hover().is_none());
        assert_eq!(link.outstanding(), 3);

        let deadline = Instant::now() + Duration::from_secs(5);
        let answer = loop {
            if let Some(answer) = link.take_answer() {
                break answer;
            }
            assert!(
                Instant::now() < deadline,
                "the ready frame answer did not arrive"
            );
            thread::yield_now();
        };
        assert!(matches!(answer.event, UiEvent::Update(_)));
        assert_eq!(
            link.outstanding_names(),
            ["frame_update", game_api::GAME_TIME, "game_hover"],
            "a stream of hover changes must not take the only place ahead of game_time"
        );
    }

    #[test]
    fn a_game_time_failure_met_after_the_loop_is_returned_by_the_late_paths_and_nothing_new_is_posted()
     {
        let mut timed = timed(
            "game.on_time(function(t) window.record(t.hour) error('late boom') end)",
            Extras::default(),
        );
        timed.slot.publish(snapshot_at(1, 1, None));
        let _ = timed.link.take_answer();
        let give_up = Instant::now() + Duration::from_secs(10);
        while timed.seen().is_empty() {
            assert!(Instant::now() < give_up, "the call did not run");
            thread::yield_now();
        }
        // The loop is over: a newer snapshot is not posted by the late paths, and the failure is returned once.
        timed.slot.publish(snapshot_at(2, 1, None));
        // The call's result may still be on its way: the fold is repeated until it has it.
        let mut failures = Vec::new();
        while failures.is_empty() {
            failures = timed.link.take_remaining_failures();
            assert!(Instant::now() < give_up, "the failure did not arrive");
            thread::yield_now();
        }
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0].starts_with("ui callback game_time failed: "),
            "{failures:?}"
        );
        assert!(failures[0].contains("late boom"), "{failures:?}");
        assert_eq!(timed.link.outstanding(), 0);
        assert_eq!(timed.seen(), ["1"]);
        assert!(timed.link.take_remaining_failures().is_empty());
    }
}
