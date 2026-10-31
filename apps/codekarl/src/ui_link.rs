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
    /// The text of the terminal failure `take_failure` returned, once.
    failure: RefCell<Option<String>>,
    /// A post reported that the VM ended.
    disconnected: Cell<bool>,
}

/// The UI link: cheap to clone, and every clone shares the handle and what the link remembers, so the
/// run closure keeps one for the late-failure steps while `ck_window::run` owns another.
#[derive(Clone)]
pub(crate) struct CodekarlUi {
    slot: UiSlot,
    state: Rc<State>,
}

impl CodekarlUi {
    /// A link over whatever handle `slot` will hold.
    pub(crate) fn new(slot: UiSlot) -> Self {
        Self {
            slot,
            state: Rc::new(State::default()),
        }
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
    }

    /// The names of the callbacks still running, oldest first.
    pub(crate) fn outstanding_names(&self) -> Vec<&'static str> {
        self.state
            .pending
            .borrow()
            .iter()
            .map(|(_, event)| name_of(event))
            .collect()
    }

    /// Takes every result that is already there, never waiting, and returns every failure among them, in
    /// order (the post-stop fold: any remaining result is taken here).
    pub(crate) fn take_remaining_failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        let mut link = self.clone();
        while let Some(answer) = link.take_answer() {
            if let Answer::Failed { message } = answer.answer {
                failures.push(message);
            }
        }
        failures
    }
}

impl UiLink for CodekarlUi {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
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
                self.state.pending.borrow_mut().push_back((id, event));
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
        let vm = self.vm()?;
        let result = vm.borrow_mut().take_result()?;
        let mut pending = self.state.pending.borrow_mut();
        let position = pending.iter().position(|(id, _)| *id == result.id)?;
        let (_, event) = pending.remove(position)?;
        drop(pending);
        let answer = answer_of(&event, result.outcome);
        Some(UiAnswer { event, answer })
    }

    fn is_disconnected(&self) -> bool {
        if self.state.failure.borrow().is_some() || self.state.disconnected.get() {
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
    use std::sync::atomic::{AtomicBool, Ordering};
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
}
