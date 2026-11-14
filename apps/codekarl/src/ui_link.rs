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
use std::sync::Arc;

use std::collections::BTreeMap;

use ck_lua::{CallId, CallOutcome, ConfigValue, VmError, VmOwner, VmState};
use ck_window::{
    Answer, CloseSource, Edge, InputTick, PointerEvent, PointerPosition, PostError, ScrollUnit,
    UiAnswer, UiEvent, UiLink, UpdateEvent,
};

use crate::frame_api::FrameRegistration;
use crate::input_stash::InputStash;
use crate::vm::UI_CALL_LIMIT;
use crate::window_api::{CLOSE_REQUEST, FRAME_UPDATE, OPENED};

/// The shared handle of the UI VM.
pub(crate) type UiVm = Rc<RefCell<VmOwner>>;

/// Where the UI VM's spawn closure puts its handle, for the run closure to find (empty when there is no
/// `ui` VM, or before the VM is spawned).
pub(crate) type UiSlot = Rc<RefCell<Option<UiVm>>>;

/// The callback name and arguments of an event, for a window whose UI is in points.
///
/// # Errors
///
/// An update whose sequence is not a Lua integer (above `i64::MAX`): the call is not built, so no callback is ever given
/// an altered identity.
#[cfg(test)]
fn call_of(
    event: &UiEvent,
    stash: &InputStash,
) -> Result<(&'static str, Vec<ConfigValue>), String> {
    call_in(event, stash, None)
}

/// [`call_of`] for a window whose UI is in UI units: an update carries `view`, the size in units, as its width and height.
fn call_in(
    event: &UiEvent,
    stash: &InputStash,
    view: Option<(u32, u32)>,
) -> Result<(&'static str, Vec<ConfigValue>), String> {
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
        UiEvent::Update(update) => (FRAME_UPDATE, vec![update_table_in(update, stash, view)?]),
    })
}

/// A window size as the integer a script sees (a `u32` always fits, so the saturating end is unreachable).
fn integer(value: impl TryInto<i64>) -> ConfigValue {
    ConfigValue::Integer(value.try_into().unwrap_or(i64::MAX))
}

fn boolean(value: bool) -> ConfigValue {
    ConfigValue::Bool(value)
}

fn pointer_number(value: i32) -> ConfigValue {
    ConfigValue::Float(f64::from(value) / 256.0)
}

fn pointer_key(key: Option<&String>) -> ConfigValue {
    key.map_or(ConfigValue::Bool(false), |key| {
        ConfigValue::String(key.clone())
    })
}

fn pointer_event_table(event: &crate::input_stash::PointerViewEvent) -> ConfigValue {
    let mut table = BTreeMap::from([
        (
            "kind".to_owned(),
            ConfigValue::String(event.kind.to_owned()),
        ),
        ("x".to_owned(), pointer_number(event.position.x)),
        ("y".to_owned(), pointer_number(event.position.y)),
        ("key".to_owned(), pointer_key(event.key.as_ref())),
        ("inside".to_owned(), ConfigValue::Bool(event.inside)),
    ]);
    if let Some(amount) = event.amount {
        table.insert(
            "amount".to_owned(),
            ConfigValue::Float(f64::from(amount) / 256.0),
        );
    }
    if let Some(unit) = event.unit {
        table.insert("unit".to_owned(), ConfigValue::String(unit.to_owned()));
    }
    if let Some(button) = event.button {
        table.insert("button".to_owned(), ConfigValue::String(button.to_owned()));
    }
    ConfigValue::Table(table)
}

/// The table a `frame_update` callback is given (design §8.2): `dt`, `elapsed` (seconds), `sequence`; the window in
/// points (`width`, `height`) and in pixels (`pixel_width`, `pixel_height`); `actions`, one entry per action name of
/// the run's bindings, unbound ones included, each `{held, pressed, released}`; `transitions`, the ordered edges as
/// `{action = name, kind = "press" | "release"}`; and `overflow`, `reset`. The input fields come from the stash (what the input
/// ticks gathered since the last accepted post); the rest from the event. Built from `ConfigValue` only (no `mlua` type), in
/// O(actions + transitions), both bounded by the stash.
///
/// The sequence is a `u64` and a script's integer an `i64`; a sequence above `i64::MAX` is an error (never saturated or
/// wrapped), which the link turns into a terminal failure.
#[cfg(test)]
fn update_table(update: &UpdateEvent, stash: &InputStash) -> Result<ConfigValue, String> {
    update_table_in(update, stash, None)
}

/// [`update_table`] with `view`, when the window's UI is in UI units, as `width` and `height`: the size the UI lays out in. The pixel
/// size is always the drawable's.
fn update_table_in(
    update: &UpdateEvent,
    stash: &InputStash,
    view: Option<(u32, u32)>,
) -> Result<ConfigValue, String> {
    let (width, height) = view.unwrap_or(update.metrics.points);
    let sequence = i64::try_from(update.sequence).map_err(|_| {
        format!(
            "frame_update sequence {} does not fit a Lua integer (at most {})",
            update.sequence,
            i64::MAX
        )
    })?;
    let actions = stash
        .actions()
        .map(|(name, action)| {
            (
                name.to_owned(),
                ConfigValue::Table(BTreeMap::from([
                    ("held".to_owned(), boolean(action.held)),
                    ("pressed".to_owned(), boolean(action.pressed)),
                    ("released".to_owned(), boolean(action.released)),
                ])),
            )
        })
        .collect();
    let transitions = stash
        .transitions()
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
    let mut fields = BTreeMap::from([
        ("dt".to_owned(), ConfigValue::Float(update.dt.as_secs_f64())),
        (
            "elapsed".to_owned(),
            ConfigValue::Float(update.elapsed.as_secs_f64()),
        ),
        ("sequence".to_owned(), ConfigValue::Integer(sequence)),
        ("width".to_owned(), integer(width)),
        ("height".to_owned(), integer(height)),
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
        ("overflow".to_owned(), boolean(stash.overflow())),
        ("reset".to_owned(), boolean(stash.reset())),
        // The typed entries (CK-314), one ordered list: `{kind = "char", text = "á"}` for a committed character, and `{kind = name}` for
        // an edit command (`"backspace"`, `"enter"`, `"tab"`, `"left"`, `"right"`). `typed_lost` says that entries were dropped at the
        // bound; `typed_epoch` is the generation of the latest `window.set_text_input` call the main thread has applied.
        (
            "typed".to_owned(),
            ConfigValue::Array(stash.typed().copied().map(typed_entry_table).collect()),
        ),
        ("typed_lost".to_owned(), boolean(stash.typed_lost())),
        (
            "typed_epoch".to_owned(),
            ConfigValue::Integer(i64::try_from(stash.typed_epoch()).unwrap_or(i64::MAX)),
        ),
    ]);
    if let Some(capture) = stash.capture() {
        fields.insert("capture".to_owned(), capture_table(capture));
    }
    if let Some(pointer) = stash.pointer() {
        fields.insert(
            "pointer".to_owned(),
            ConfigValue::Table(BTreeMap::from([
                ("x".to_owned(), pointer_number(pointer.position.x)),
                ("y".to_owned(), pointer_number(pointer.position.y)),
                ("held".to_owned(), boolean(pointer.held)),
                ("over".to_owned(), pointer_key(pointer.over.as_ref())),
                ("overflow".to_owned(), boolean(stash.pointer_overflow())),
                (
                    "events".to_owned(),
                    ConfigValue::Array(stash.pointer_events().map(pointer_event_table).collect()),
                ),
            ])),
        );
    }
    Ok(ConfigValue::Table(fields))
}

fn capture_table(capture: &ck_window::CaptureResult) -> ConfigValue {
    let (kind, value) = match &capture.outcome {
        ck_window::CaptureOutcome::Binding(binding) => {
            ("binding", ConfigValue::String(binding.clone()))
        }
        ck_window::CaptureOutcome::Cancelled(reason) => (
            "cancelled",
            ConfigValue::String(
                match reason {
                    ck_window::CaptureCancelReason::Replaced => "replaced",
                    ck_window::CaptureCancelReason::Requested => "requested",
                    ck_window::CaptureCancelReason::Focus => "focus",
                }
                .to_owned(),
            ),
        ),
    };
    ConfigValue::Table(BTreeMap::from([
        ("id".to_owned(), ConfigValue::String(capture.id.clone())),
        ("kind".to_owned(), ConfigValue::String(kind.to_owned())),
        ("value".to_owned(), value),
    ]))
}

/// One typed entry as the update's table.
fn typed_entry_table(entry: ck_window::TypedEntry) -> ConfigValue {
    use ck_window::TypedEntry;
    let (kind, text) = match entry {
        TypedEntry::Char(character) => ("char", Some(character.to_string())),
        TypedEntry::Backspace => ("backspace", None),
        TypedEntry::Enter => ("enter", None),
        TypedEntry::Tab => ("tab", None),
        TypedEntry::Left => ("left", None),
        TypedEntry::Right => ("right", None),
    };
    let mut fields = BTreeMap::from([("kind".to_owned(), ConfigValue::String(kind.to_owned()))]);
    if let Some(text) = text {
        fields.insert("text".to_owned(), ConfigValue::String(text));
    }
    ConfigValue::Table(fields)
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
    /// Each with the economy revision its snapshot carried (`0` for none).
    times: RefCell<Vec<(CallId, u64)>>,
    /// The `game_hover` calls posted and not yet answered.
    hovers: RefCell<Vec<CallId>>,
    /// The `game_select` calls posted and not yet answered.
    selections: RefCell<Vec<CallId>>,
    /// Guarded-load outcomes waiting for a free callback place and the outcomes already posted.
    guarded_results: RefCell<VecDeque<crate::game::controls::GuardedLoadResult>>,
    guarded_calls: RefCell<Vec<CallId>>,
    /// The snapshot last delivered through `game_time`, to post the next only when something displayed differs.
    delivered: RefCell<Option<crate::game::slot::SimSnapshot>>,
    /// Hover last posted; outer `None` means callback registration changed and needs a fresh delivery.
    delivered_hover: RefCell<DeliveredHover>,
    /// Selection sequence last posted; cleared when no callback is registered.
    delivered_selection: Cell<Option<u64>>,
    /// Which latest-wins channel gets the next free non-lifecycle call place.
    next_update: Cell<UpdateChannel>,
    /// The text of the terminal failure `take_failure` returned, once.
    failure: RefCell<Option<String>>,
    /// A post reported that the VM ended.
    disconnected: Cell<bool>,
    /// The latest pointer position in window points, kept for the map's hover when a tick carries none (the stash holds the UI's, in UI
    /// units in a window opened in pixels).
    last_points: Cell<Option<PointerPosition>>,
    /// The metrics of the latest input tick: what converts the map's hover positions to the UI's units when they are delivered.
    last_metrics: Cell<Option<ck_window::WindowMetrics>>,
    /// The input the UI VM has not been given yet; cleared when a post carrying it was accepted.
    stash: RefCell<InputStash>,
    /// The `frame.on_update` registration generation the previous input tick saw.
    seen_generation: Cell<u64>,
    /// The owner of the current pointer gesture, independent of what input detail survived overflow.
    pointer_owner: RefCell<GestureOwner>,
    /// Whether the previous input tick saw the game-key block on, to end a captured gesture on the tick it turns on.
    keys_were_blocked: Cell<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum GestureOwner {
    #[default]
    Idle,
    Ui(String),
    Map,
    Nobody,
}

#[derive(Default)]
struct RoutedPointer {
    map: Vec<crate::map_app::RoutedMapEvent>,
    ui: Vec<crate::input_stash::PointerViewEvent>,
}

fn view_event(
    kind: &'static str,
    position: PointerPosition,
    key: Option<String>,
) -> crate::input_stash::PointerViewEvent {
    crate::input_stash::PointerViewEvent {
        kind,
        position,
        key,
        inside: false,
        button: None,
        amount: None,
        unit: None,
    }
}

/// [`route_pointer_in`] for a window whose UI is in points.
#[cfg(test)]
fn route_pointer(
    owner: &mut GestureOwner,
    input: &ck_window::InputSnapshot,
    regions: Option<&ck_render_2d::HitRegions>,
) -> RoutedPointer {
    route_pointer_in(owner, input, regions, None)
}

/// [`route_pointer_with`] when no gesture is ended from outside.
#[cfg(test)]
fn route_pointer_in(
    owner: &mut GestureOwner,
    input: &ck_window::InputSnapshot,
    regions: Option<&ck_render_2d::HitRegions>,
    units: Option<ck_render_2d::ui_scale::UnitMap>,
) -> RoutedPointer {
    route_pointer_with(owner, input, regions, units, false)
}

/// Routes one tick's pointer input. `end_capture` ends the gesture someone already owns, as a focus loss does: the pause menu's
/// backdrop is a hit region above everything, but a drag the map (or a widget) captured before it opened would otherwise go on, so
/// the tick the menu's key block turns on ends it. The rest of that button press then reaches nobody.
#[expect(
    clippy::too_many_lines,
    reason = "gesture ownership and per-event routing form one ordered state transition"
)]
fn route_pointer_with(
    owner: &mut GestureOwner,
    input: &ck_window::InputSnapshot,
    regions: Option<&ck_render_2d::HitRegions>,
    units: Option<ck_render_2d::ui_scale::UnitMap>,
    end_capture: bool,
) -> RoutedPointer {
    use crate::map_app::RoutedMapEvent as MapEvent;
    let mut routed = RoutedPointer::default();
    // In a window opened in pixels the UI's positions, hit tests and scroll amounts are in UI units; the map's events keep their points.
    let ui = |position: PointerPosition| units.map_or(position, |map| map.position(position));
    let latest = ui(input.pointer_position.unwrap_or_default());
    if matches!(owner, GestureOwner::Nobody) && !input.pointer_held {
        *owner = GestureOwner::Idle;
    }
    if input.reset || end_capture {
        match owner {
            GestureOwner::Ui(key) => {
                routed
                    .ui
                    .push(view_event("cancel", latest, Some(key.clone())));
            }
            GestureOwner::Map => routed.map.push(MapEvent::EndDrag),
            GestureOwner::Idle | GestureOwner::Nobody => {}
        }
        if !matches!(owner, GestureOwner::Idle) {
            *owner = GestureOwner::Nobody;
        }
    } else if let GestureOwner::Ui(key) = owner
        && !regions.is_some_and(|regions| regions.contains_key(key))
    {
        routed
            .ui
            .push(view_event("cancel", latest, Some(key.clone())));
        *owner = GestureOwner::Nobody;
    }

    for event in &input.pointer_events {
        match *event {
            PointerEvent::Down(position) => {
                match owner {
                    GestureOwner::Map => routed.map.push(MapEvent::EndDrag),
                    GestureOwner::Ui(key) => {
                        routed
                            .ui
                            .push(view_event("cancel", ui(position), Some(key.clone())));
                    }
                    GestureOwner::Idle | GestureOwner::Nobody => {}
                }
                if let Some(key) = regions.and_then(|regions| regions.key_at(ui(position))) {
                    let key = key.to_owned();
                    routed
                        .ui
                        .push(view_event("down", ui(position), Some(key.clone())));
                    *owner = GestureOwner::Ui(key);
                } else {
                    routed.map.push(MapEvent::Pointer(*event));
                    // The UI does not own the map gesture, but tooltips need the actual left press to close unlocked panels.
                    routed.ui.push(view_event("map_down", ui(position), None));
                    *owner = GestureOwner::Map;
                }
            }
            PointerEvent::ButtonDown { button, position } => {
                let key = regions
                    .and_then(|regions| regions.key_at(ui(position)))
                    .map(str::to_owned);
                let mut event = view_event("button_down", ui(position), key);
                event.button = Some(match button {
                    ck_window::PointerButton::Middle => "middle",
                    ck_window::PointerButton::Right => "right",
                });
                routed.ui.push(event);
            }
            PointerEvent::ButtonUp { button, position } => {
                let key = regions
                    .and_then(|regions| regions.key_at(ui(position)))
                    .map(str::to_owned);
                let mut event = view_event("button_up", ui(position), key);
                event.button = Some(match button {
                    ck_window::PointerButton::Middle => "middle",
                    ck_window::PointerButton::Right => "right",
                });
                routed.ui.push(event);
            }
            PointerEvent::Move(position) => match owner {
                GestureOwner::Ui(key) => {
                    routed
                        .ui
                        .push(view_event("move", ui(position), Some(key.clone())));
                }
                GestureOwner::Map => routed.map.push(MapEvent::Pointer(*event)),
                GestureOwner::Idle => {
                    let key = regions
                        .and_then(|regions| regions.key_at(ui(position)))
                        .map(str::to_owned);
                    routed
                        .ui
                        .push(view_event("move", ui(position), key.clone()));
                    if key.is_none() {
                        routed.map.push(MapEvent::Pointer(*event));
                    }
                }
                GestureOwner::Nobody => {}
            },
            PointerEvent::Up(position) => {
                match owner {
                    GestureOwner::Ui(key) => {
                        routed
                            .ui
                            .push(view_event("up", ui(position), Some(key.clone())));
                    }
                    GestureOwner::Map => routed.map.push(MapEvent::Pointer(*event)),
                    GestureOwner::Idle | GestureOwner::Nobody => {}
                }
                if matches!(owner, GestureOwner::Ui(_) | GestureOwner::Map) {
                    *owner = GestureOwner::Idle;
                }
            }
            PointerEvent::Scroll {
                position,
                amount,
                unit,
            } => {
                let key = match owner {
                    GestureOwner::Ui(key) => Some(key.clone()),
                    GestureOwner::Idle | GestureOwner::Map | GestureOwner::Nobody => regions
                        .and_then(|regions| regions.key_at(ui(position)))
                        .map(str::to_owned),
                };
                if key.is_some() {
                    let mut view = view_event("scroll", ui(position), key);
                    view.amount = Some(match (unit, units) {
                        (ScrollUnit::Points, Some(map)) => map.scroll_amount(amount),
                        _ => amount,
                    });
                    view.unit = Some(match unit {
                        ScrollUnit::Lines => "lines",
                        ScrollUnit::Points => "points",
                    });
                    routed.ui.push(view);
                } else if !matches!(owner, GestureOwner::Ui(_)) {
                    routed.map.push(MapEvent::Pointer(*event));
                }
            }
        }
    }

    if input.pointer_overflow {
        match owner {
            GestureOwner::Ui(key) if input.pointer_held => {
                if let Some(position) = input.pointer_position {
                    routed
                        .ui
                        .push(view_event("move", ui(position), Some(key.clone())));
                }
            }
            GestureOwner::Ui(key) => {
                routed
                    .ui
                    .push(view_event("cancel", latest, Some(key.clone())));
                *owner = GestureOwner::Nobody;
            }
            GestureOwner::Idle if input.pointer_held => *owner = GestureOwner::Nobody,
            GestureOwner::Map | GestureOwner::Idle | GestureOwner::Nobody => {}
        }
    } else if matches!(owner, GestureOwner::Idle) && input.pointer_held {
        // A held button without a retained Down has no owner.
        *owner = GestureOwner::Nobody;
    }
    if matches!(owner, GestureOwner::Nobody) && !input.pointer_held {
        *owner = GestureOwner::Idle;
    }
    // Each event says whether it lay inside one of its key's published rectangles, decided here, once, against the very regions this
    // tick routed with: the widget never recomputes geometry against a frame the player was not pointing at.
    for event in &mut routed.ui {
        event.inside = event
            .key
            .as_deref()
            .zip(regions)
            .is_some_and(|(key, regions)| regions.key_contains(key, event.position));
    }
    routed
}

#[derive(Default)]
enum DeliveredHover {
    #[default]
    Unsent,
    /// The hover and the conversion it was delivered with: a hover the UI has in units is stale when the conversion changes (a display or
    /// scale change) though the map's own hover did not.
    Sent(
        Option<crate::map_app::Hover>,
        Option<ck_render_2d::ui_scale::UnitMap>,
    ),
}

impl DeliveredHover {
    /// Whether what the UI last got is still what it should have: the same map hover, converted the same way. A hover in UI units is stale
    /// when the conversion changed (a display or scale change) though the map's own hover did not; no hover has no conversion to be stale.
    fn is_current(
        &self,
        hover: Option<&crate::map_app::Hover>,
        units: Option<ck_render_2d::ui_scale::UnitMap>,
    ) -> bool {
        match self {
            Self::Unsent => false,
            Self::Sent(delivered, conversion) => {
                delivered.as_ref() == hover && (hover.is_none() || *conversion == units)
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
enum UpdateChannel {
    #[default]
    Time,
    Hover,
    Select,
}

impl UpdateChannel {
    fn next(self) -> Self {
        match self {
            Self::Time => Self::Hover,
            Self::Hover => Self::Select,
            Self::Select => Self::Time,
        }
    }
}

/// The UI link: cheap to clone, and every clone shares the handle and what the link remembers, so the
/// run closure keeps one for the late-failure steps while `ck_window::run` owns another.
#[derive(Clone)]
pub(crate) struct CodekarlUi {
    slot: UiSlot,
    state: Rc<State>,
    map: Option<crate::map_app::SharedController>,
    /// The scene publication source, even when this run has no map controller.
    scene: Option<Arc<ck_render_2d::SceneBridge>>,
    sim: Option<SimLink>,
    /// Whether the UI VM has a `frame_update` callback, as the setter's observer published it; `None` counts as registered.
    registration: Option<FrameRegistration>,
}

/// The simulation owner as the UI link sees it: where presses go, and what it published.
#[derive(Clone)]
pub(crate) struct SimLink {
    pub(crate) controls: crate::game::controls::SimControls,
    pub(crate) slot: Arc<crate::game::slot::SimSlot>,
    /// Whether the UI VM has registered a `game.on_time` callback.
    pub(crate) time_registered: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the UI VM has registered a `game.on_hover` callback.
    pub(crate) hover_registered: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the UI VM has registered a `game.on_select` callback.
    pub(crate) select_registered: Arc<std::sync::atomic::AtomicBool>,
    /// The economy views `game.place` reads: each snapshot's view goes in before the `game_time` call that carries its revision.
    pub(crate) views: Arc<crate::game_api::ViewCache>,
}

impl CodekarlUi {
    /// A link over whatever handle `slot` will hold.
    pub(crate) fn new(slot: UiSlot) -> Self {
        Self {
            slot,
            state: Rc::new(State::default()),
            map: None,
            scene: None,
            sim: None,
            registration: None,
        }
    }

    pub(crate) fn with_map(slot: UiSlot, map: crate::map_app::SharedController) -> Self {
        Self {
            slot,
            state: Rc::new(State::default()),
            map: Some(map),
            scene: None,
            sim: None,
            registration: None,
        }
    }

    /// Attach the scene publication source used for hit-region routing, including in map-less runs.
    pub(crate) fn with_scene_bridge(mut self, scene: Arc<ck_render_2d::SceneBridge>) -> Self {
        self.scene = Some(scene);
        self
    }

    /// This link reading the UI VM's `frame.on_update` registration from `registration`, which the setter's observer writes.
    pub(crate) fn with_registration(mut self, registration: FrameRegistration) -> Self {
        self.registration = Some(registration);
        self
    }

    /// This link with the simulation owner attached: it carries the update's time presses to the owner, and ends the run when
    /// the owner records a terminal failure.
    pub(crate) fn with_sim(mut self, sim: SimLink) -> Self {
        self.sim = Some(sim);
        self
    }

    /// The pause menu's key block: its backdrop takes every new press, so the tick the block turns on (the first returned value) also ends
    /// a gesture someone had already captured, and while it is on the game's own actions stop reaching the map and the simulation (the
    /// second: the input without them; the UI still gets all of it).
    fn key_block(
        &self,
        input: &ck_window::InputSnapshot,
    ) -> (bool, Option<ck_window::InputSnapshot>) {
        let blocked = self
            .sim
            .as_ref()
            .is_some_and(|sim| sim.controls.keys_blocked());
        // Always store this tick's state: a short-circuit here would leave an old `true` behind when the block turns off.
        let was_blocked = self.state.keys_were_blocked.replace(blocked);
        let began = blocked && !was_blocked;
        (
            began,
            blocked.then(|| crate::game::controls::without_blocked_actions(input)),
        )
    }

    /// Sends the presses of `input` to the simulation owner, and writes the one record when the input layer discarded earlier transitions.
    fn send_controls(&self, input: &ck_window::InputSnapshot) {
        if let Some(sim) = &self.sim
            && sim.controls.send(input)
        {
            crate::lifecycle::emit(
                ck_log::Level::Warn,
                "sim",
                format_args!("{}", crate::game::controls::OVERFLOW_RECORD),
            );
        }
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
            + self.state.selections.borrow().len()
            + self.state.guarded_calls.borrow().len()
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
        names.extend(
            self.state
                .selections
                .borrow()
                .iter()
                .map(|_| crate::game_api::GAME_SELECT),
        );
        names.extend(
            self.state
                .guarded_calls
                .borrow()
                .iter()
                .map(|_| crate::game_api::GAME_SAVE_RESULT),
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
    /// Reads the `frame.on_update` registration once and applies a change of it to `stash`: when the generation differs from the one
    /// last seen, everything that was an event is dropped (each action's latest `held` stays), whether the new state is registered or
    /// not. Returns whether a callback is registered. Called by every tick and by every update post, so the boundary is the first of
    /// them that observes the new generation, and an update posted before the next tick (the control epoch restarts the UI cursor
    /// at once) can never carry what an earlier registration gathered. With no registration attached the callback counts as
    /// registered, always.
    fn observe_registration(&self, stash: &mut InputStash) -> bool {
        let Some(registration) = &self.registration else {
            return true;
        };
        let (generation, registered) = registration.state();
        if generation != self.state.seen_generation.get() {
            stash.clear();
            self.state.seen_generation.set(generation);
        }
        registered
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
            *self.state.delivered.borrow_mut() = None;
            return false;
        }
        let Some(snapshot) = sim.slot.latest() else {
            return false;
        };
        if self
            .state
            .delivered
            .borrow()
            .as_ref()
            .is_some_and(|delivered| !snapshot.differs_from(delivered))
        {
            return false;
        }
        if self.outstanding() >= UI_CALL_LIMIT - 1 {
            return false;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        // The call's own view is in the cache, pinned by the call, before the call is posted: the call can read it with `game.place`
        // until a later call completes.
        let pinned = snapshot.economy.as_ref().map(|view| {
            sim.views.post(snapshot.economy_revision, Arc::clone(view));
            snapshot.economy_revision
        });
        let posted = vm.borrow_mut().post(
            crate::game_api::GAME_TIME,
            vec![crate::game_api::time_table(&snapshot)],
        );
        if let Ok(id) = posted {
            self.state
                .times
                .borrow_mut()
                .push((id, snapshot.economy_revision));
            *self.state.delivered.borrow_mut() = Some(snapshot);
            return true;
        }
        // No call exists: nothing pins the view it was to be read by.
        if let Some(revision) = pinned {
            sim.views.unpost(revision);
        }
        false
    }

    /// The conversion of the map's hover positions (window points) to UI units, for a window opened in pixels, from the latest tick's metrics.
    fn hover_units(&self) -> Option<ck_render_2d::ui_scale::UnitMap> {
        self.unit_map(self.state.last_metrics.get()?)
    }

    /// The conversion of window points to UI units for `metrics`, when the window was opened in pixels (its UI is in UI units at the scale
    /// the drawable allows); `None` when it was opened in points.
    fn unit_map(
        &self,
        metrics: ck_window::WindowMetrics,
    ) -> Option<ck_render_2d::ui_scale::UnitMap> {
        let k = self.scene.as_ref()?.ui_scale(metrics.pixels)?;
        Some(ck_render_2d::ui_scale::UnitMap {
            pixels: metrics.pixels,
            points: metrics.points,
            k,
        })
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
        let units = self.hover_units();
        if self
            .state
            .delivered_hover
            .borrow()
            .is_current(hover.as_ref(), units)
            || !self.state.hovers.borrow().is_empty()
            || self.outstanding() >= UI_CALL_LIMIT - 1
        {
            return false;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        if let Ok(id) = vm.borrow_mut().post(
            crate::game_api::GAME_HOVER,
            crate::game_api::hover_arguments(hover.as_ref(), units),
        ) {
            self.state.hovers.borrow_mut().push(id);
            *self.state.delivered_hover.borrow_mut() = DeliveredHover::Sent(hover, units);
            return true;
        }
        false
    }

    /// Posts only the current selection when its sequence has not been delivered to the registered callback.
    fn deliver_select(&self) -> bool {
        let (Some(map), Some(sim)) = (&self.map, &self.sim) else {
            return false;
        };
        if !sim
            .select_registered
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.state.delivered_selection.set(None);
            return false;
        }
        let Some(selection) = crate::map_app::lock(map).selection() else {
            return false;
        };
        if self.state.delivered_selection.get() == Some(selection.seq)
            || !self.state.selections.borrow().is_empty()
            || self.outstanding() >= UI_CALL_LIMIT - 1
        {
            return false;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        if let Ok(id) = vm.borrow_mut().post(
            crate::game_api::GAME_SELECT,
            crate::game_api::select_arguments(&selection),
        ) {
            self.state.selections.borrow_mut().push(id);
            self.state.delivered_selection.set(Some(selection.seq));
            return true;
        }
        false
    }

    /// Posts one completed guarded-load result without waiting on the simulation owner.
    fn deliver_guarded_result(&self) -> bool {
        let Some(sim) = &self.sim else {
            return false;
        };
        while let Some(result) = sim.controls.take_guarded_result() {
            self.state.guarded_results.borrow_mut().push_back(result);
        }
        if !sim.controls.save_result_registered()
            || self.outstanding() >= UI_CALL_LIMIT - 1
            || self.state.guarded_results.borrow().is_empty()
        {
            return false;
        }
        let Some(vm) = self.vm() else {
            return false;
        };
        let result = self
            .state
            .guarded_results
            .borrow()
            .front()
            .cloned()
            .expect("the queue was checked above");
        match vm.borrow_mut().post(
            crate::game_api::GAME_SAVE_RESULT,
            crate::game_api::guarded_result_arguments(&result),
        ) {
            Ok(id) => {
                self.state.guarded_results.borrow_mut().pop_front();
                self.state.guarded_calls.borrow_mut().push(id);
                true
            }
            Err(VmError::Full { .. }) => false,
            Err(_) => {
                self.state.disconnected.set(true);
                false
            }
        }
    }

    /// Gives each changed latest-wins channel a chance at each available place. When only one place
    /// is free, the channel that did not get the previous successful delivery goes first.
    fn deliver_updates(&self) {
        self.deliver_guarded_result();
        let first = self.state.next_update.get();
        let mut channel = first;
        for _ in 0..3 {
            let accepted = match channel {
                UpdateChannel::Time => self.deliver_time(),
                UpdateChannel::Hover => self.deliver_hover(),
                UpdateChannel::Select => self.deliver_select(),
            };
            if accepted {
                self.state.next_update.set(channel.next());
            }
            channel = channel.next();
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
                    .position(|(id, _)| *id == result.id)
                    .map(|position| times.remove(position))
            };
            if let Some((_, revision)) = timed {
                // This call has completed: older views that no pending call pins are retired.
                if revision != 0
                    && let Some(sim) = &self.sim
                {
                    sim.views.complete(revision);
                }
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
            let selected = {
                let mut selections = self.state.selections.borrow_mut();
                selections
                    .iter()
                    .position(|id| *id == result.id)
                    .map(|position| selections.remove(position))
            };
            if selected.is_some() {
                if let CallOutcome::Failed { message, traceback } = result.outcome {
                    return Some(Taken::SelectFailed(format!(
                        "ui callback {} failed: {message}\n{traceback}",
                        crate::game_api::GAME_SELECT
                    )));
                }
                continue;
            }
            let guarded = {
                let mut calls = self.state.guarded_calls.borrow_mut();
                calls
                    .iter()
                    .position(|id| *id == result.id)
                    .map(|position| calls.remove(position))
            };
            if guarded.is_some() {
                if let CallOutcome::Failed { message, traceback } = result.outcome {
                    return Some(Taken::SaveResultFailed(format!(
                        "ui callback {} failed: {message}\n{traceback}",
                        crate::game_api::GAME_SAVE_RESULT
                    )));
                }
                continue;
            }
            let mut pending = self.state.pending.borrow_mut();
            let position = pending.iter().position(|(id, _)| *id == result.id)?;
            let (_, event) = pending.remove(position)?;
            drop(pending);
            let answer = answer_of(&event, result.outcome);
            return Some(Taken::Answer(UiAnswer { event, answer }));
        }
    }

    /// The next answer for the window loop. A failing `game_time` call is the link's terminal failure here, which ends the run
    /// through the ordinary UI-failure path.
    fn next_answer(&mut self) -> Option<UiAnswer> {
        loop {
            match self.take_next()? {
                Taken::Answer(answer) => return Some(answer),
                Taken::TimeFailed(message)
                | Taken::HoverFailed(message)
                | Taken::SelectFailed(message)
                | Taken::SaveResultFailed(message) => {
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
            | Taken::HoverFailed(message)
            | Taken::SelectFailed(message)
            | Taken::SaveResultFailed(message) => vec![message],
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
    /// A `game_select` call failed.
    SelectFailed(String),
    /// A `game_save_result` callback failed.
    SaveResultFailed(String),
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
        if matches!(event, UiEvent::Update(_)) {
            self.observe_registration(&mut self.state.stash.borrow_mut());
        }
        let view = match &event {
            UiEvent::Update(update) => self
                .scene
                .as_ref()
                .and_then(|scene| scene.ui_scale(update.metrics.pixels))
                .map(|k| ck_render_2d::ui_scale::view_units(update.metrics.pixels, k)),
            _ => None,
        };
        let (callback, arguments) = match call_in(&event, &self.state.stash.borrow(), view) {
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
                // Only the VM's acceptance clears the stash: a refused post leaves it for the next one. The map and the time
                // controls took this input at its tick, so a retried post can never repeat them.
                if matches!(event, UiEvent::Update(_)) {
                    self.state.stash.borrow_mut().clear();
                }
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

    /// One input tick, on the main thread, never waiting: the map controller and the time controls take it, and the UI's share goes
    /// to the stash (trimmed as the `frame.on_update` registration says).
    fn input(&mut self, tick: &InputTick) -> Result<(), String> {
        let regions = if let Some(scene) = &self.scene {
            scene.hit_regions()
        } else {
            self.map
                .as_ref()
                .and_then(|map| crate::map_app::lock(map).hit_regions())
        };
        // In a window opened in pixels the UI works in UI units: the scale follows the drawable, and one conversion serves the Lua events, the
        // hit tests and the hover. The map's own input stays in points.
        let units = self.unit_map(tick.metrics);
        self.state.last_metrics.set(Some(tick.metrics));
        let previous_ui = self
            .state
            .stash
            .borrow()
            .pointer()
            .map(|pointer| pointer.position);
        let previous_points = self.state.last_points.get();
        let latest_points = tick.input.pointer_position.or(previous_points);
        self.state.last_points.set(latest_points);
        let latest_position = tick
            .input
            .pointer_position
            .map(|position| units.map_or(position, |map| map.position(position)))
            .or(previous_ui);
        let (block_began, game_input) = self.key_block(&tick.input);
        let mut owner = self.state.pointer_owner.borrow_mut();
        let routed = route_pointer_with(
            &mut owner,
            &tick.input,
            regions.as_deref(),
            units,
            block_began,
        );
        let latest_over = latest_position
            .and_then(|position| {
                regions
                    .as_deref()
                    .and_then(|regions| regions.key_at(position))
            })
            .map(str::to_owned);
        let map_owns_gesture = matches!(*owner, GestureOwner::Map);
        let suppress_hover = tick.input.reset
            || tick.input.pointer_held
            || matches!(*owner, GestureOwner::Ui(_))
            || latest_over.is_some();
        if let Some(map) = &self.map {
            let mut map_tick = tick.clone();
            if let Some(filtered) = &game_input {
                map_tick.input.actions.clone_from(&filtered.actions);
            }
            map_tick.input.pointer_events.clear();
            map_tick.input.pointer_position = map_owns_gesture
                .then_some(tick.input.pointer_position)
                .flatten();
            map_tick.input.pointer_held = map_owns_gesture && tick.input.pointer_held;
            map_tick.input.pointer_overflow = map_owns_gesture && tick.input.pointer_overflow;
            let hover_position = if !suppress_hover && !map_owns_gesture {
                latest_points
            } else {
                None
            };
            crate::map_app::lock(map).update_routed(
                &map_tick,
                &routed.map,
                hover_position,
                suppress_hover,
                tick.input.pointer_overflow,
            )?;
        }
        if map_owns_gesture && tick.input.pointer_overflow && !tick.input.pointer_held {
            *owner = GestureOwner::Idle;
        }
        self.send_controls(game_input.as_ref().unwrap_or(&tick.input));
        let mut stash = self.state.stash.borrow_mut();
        // A registration since the previous look at it (a tick or a post): what was gathered before it is never delivered.
        let registered = self.observe_registration(&mut stash);
        if registered {
            stash.absorb(&tick.input);
            stash.absorb_pointer(
                latest_position.map(|position| crate::input_stash::PointerView {
                    position,
                    held: tick.input.pointer_held,
                    over: latest_over,
                }),
                routed.ui,
                tick.input.pointer_overflow,
            );
        } else {
            // No callback: the stash follows only each action's held state, and everything that was an event is dropped.
            stash.follow_held(&tick.input);
            stash.absorb_pointer(
                latest_position.map(|position| crate::input_stash::PointerView {
                    position,
                    held: tick.input.pointer_held,
                    over: latest_over,
                }),
                [],
                false,
            );
        }
        Ok(())
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
mod glossary_reader_tests;
#[cfg(test)]
mod group_tests;
#[cfg(test)]
mod input_tick_tests;
#[cfg(test)]
mod list_tests;
#[cfg(test)]
mod listbar_tests;
#[cfg(test)]
mod location_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod scrollbar_tests;
#[cfg(test)]
mod theme_tests;
#[cfg(test)]
mod typing_tests;
#[cfg(test)]
mod widget_witness_tests;

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
        let none = InputStash::default();
        assert_eq!(call_of(&UiEvent::Opened, &none), Ok(("opened", Vec::new())));
        for (source, text) in [(CloseSource::Button, "button"), (CloseSource::Key, "key")] {
            assert_eq!(
                call_of(&UiEvent::CloseRequested { source }, &none),
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
    fn a_close_callback_step_marker_is_emitted_only_on_the_following_update() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                if let Some(Some(ConfigValue::String(value))) = arguments.first() {
                    seen.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(value.clone());
                }
                Ok(Vec::new())
            })
        };
        let harness = Harness::new(
            "local close_pending = false\n\
             window.on_close_request(function()\n\
               window.record('close-callback')\n\
               close_pending = true\n\
               return false\n\
             end)\n\
             frame.on_update(function()\n\
               if close_pending then\n\
                 window.record('step-marker')\n\
                 close_pending = false\n\
               end\n\
             end)",
            Extras {
                functions: vec![("record", record)],
                extensions: vec![frame_extension()],
                ..Extras::default()
            },
        );
        let mut link = harness.ui.clone();
        let close = UiEvent::CloseRequested {
            source: CloseSource::Key,
        };
        link.post(close.clone()).expect("close request posted");
        let closed = answer(&mut link);
        assert_eq!((closed.event, closed.answer), (close, Answer::Decline));
        assert_eq!(
            &*seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &["close-callback"],
            "the close callback returned without emitting the driver's next-step marker"
        );

        let update = update(1);
        link.post(update.clone()).expect("following update posted");
        let updated = answer(&mut link);
        assert_eq!((updated.event, updated.answer), (update, Answer::Returned));
        assert_eq!(
            &*seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            &["close-callback", "step-marker"],
            "the marker is emitted only when the next frame update runs"
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
                        crate::game::controls::SimControls::new(std::sync::mpsc::channel().0),
                    ),
                ],
                ..Extras::default()
            },
        );
        let map = crate::map_app::tests::window_case_hover_controller();
        // The Cocoa helper injects at the fixture marker's center in window coordinates.
        // The window adapter reports top-left coordinates to the map controller.
        let pointer = PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let map = crate::map_app::shared(map);
        let scene = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut writer = scene.take_writer().expect("a scene writer");
        writer
            .set_hit(
                "panel",
                Some(vec![ck_render_2d::PointRect::new(300.0, 10.0, 40.0, 40.0)]),
                None,
            )
            .expect("a valid UI hit region");
        assert!(writer.publish());
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map))
            .with_scene_bridge(Arc::clone(&scene))
            .with_sim(SimLink {
                views: Arc::default(),
                controls: crate::game::controls::SimControls::new(sender),
                slot,
                time_registered: registered,
                hover_registered,
                select_registered: Arc::new(AtomicBool::new(false)),
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
        let mut first_input = known_input();
        let UiEvent::Update(first) = &mut first else {
            unreachable!();
        };
        first.metrics.points = (400, 240);
        first.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        first_input.pointer_position = Some(pointer);
        first_input.pointer_events = vec![
            PointerEvent::Scroll {
                position: pointer,
                amount: 15 * 256,
                unit: ScrollUnit::Points,
            },
            PointerEvent::Move(pointer),
        ];
        first_input.actions = vec![action("map_pan_left", false, true, true)];
        first_input.transitions = vec![
            Transition {
                action: "map_pan_left".to_owned(),
                edge: Edge::Press,
            },
            Transition {
                action: "map_pan_left".to_owned(),
                edge: Edge::Release,
            },
        ];
        post_after_tick(&mut link, UiEvent::Update(first.clone()), first_input)
            .expect("first update posted");

        // Model more dropped owner input arriving after the pointer move, before the link gets a
        // chance to publish the controller's hover to the UI VM.
        let mut interleaved_keys = update(2);
        let mut interleaved_keys_input = known_input();
        let UiEvent::Update(interleaved_keys) = &mut interleaved_keys else {
            unreachable!();
        };
        interleaved_keys.metrics.points = (400, 240);
        interleaved_keys.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        interleaved_keys_input.pointer_position = Some(pointer);
        interleaved_keys_input.actions = vec![action("map_zoom_out", false, true, true)];
        interleaved_keys_input.transitions = vec![
            Transition {
                action: "map_zoom_out".to_owned(),
                edge: Edge::Press,
            },
            Transition {
                action: "map_zoom_out".to_owned(),
                edge: Edge::Release,
            },
        ];
        post_after_tick(
            &mut link,
            UiEvent::Update(interleaved_keys.clone()),
            interleaved_keys_input,
        )
        .expect("interleaved key update posted");

        let _ = answer(&mut link);
        wait_for_count(&mut link, 1);
        assert_eq!(seen.lock().unwrap().as_slice(), ["Fixture Town"]);

        // A middle press and release at the already-hovered position are ordinary position updates: they neither suppress the map
        // hover nor post another game.on_hover change.
        for event in [
            PointerEvent::ButtonDown {
                button: ck_window::PointerButton::Middle,
                position: pointer,
            },
            PointerEvent::ButtonUp {
                button: ck_window::PointerButton::Middle,
                position: pointer,
            },
        ] {
            let mut input = known_input();
            input.pointer_position = Some(pointer);
            input.pointer_events = vec![event];
            let mut tick = known_tick();
            tick.metrics.points = (400, 240);
            tick.metrics.pixels = PixelSize {
                width: 800,
                height: 480,
            };
            tick.input = input;
            link.input(&tick)
                .expect("the same-place button event is consumed");
            link.deliver_hover();
            let deadline = Instant::now() + Duration::from_secs(5);
            while link.outstanding() > 0 {
                let _ = link.take_answer();
                assert!(
                    Instant::now() < deadline,
                    "button hover intake did not settle"
                );
                thread::yield_now();
            }
            assert_eq!(
                crate::map_app::lock(&map)
                    .hover()
                    .as_ref()
                    .map(|hover| hover.name.as_str()),
                Some("Fixture Town"),
                "button events keep the map's existing hover"
            );
        }
        assert_eq!(seen.lock().unwrap().as_slice(), ["Fixture Town"]);

        // At a new position over UI, right-down and Move produce the same `over` and map-hover suppression result.
        let ui_position = PointerPosition {
            x: 310 * 256,
            y: 20 * 256,
        };
        let mut observed = None;
        for event in [
            PointerEvent::ButtonDown {
                button: ck_window::PointerButton::Right,
                position: ui_position,
            },
            PointerEvent::Move(ui_position),
        ] {
            let mut input = known_input();
            input.pointer_position = Some(ui_position);
            input.pointer_events = vec![event];
            let mut tick = known_tick();
            tick.metrics.points = (400, 240);
            tick.metrics.pixels = PixelSize {
                width: 800,
                height: 480,
            };
            tick.input = input;
            link.input(&tick)
                .expect("the UI-position event is consumed");
            let pointer_over = link
                .state
                .stash
                .borrow()
                .pointer()
                .and_then(|pointer| pointer.over.clone());
            let map_hover = crate::map_app::lock(&map).hover();
            assert_eq!(pointer_over.as_deref(), Some("panel"));
            let current = (pointer_over, map_hover);
            if let Some(previous) = observed {
                assert_eq!(
                    current, previous,
                    "a button edge is an ordinary pointer-position update"
                );
            }
            observed = Some(current);
        }
        assert_eq!(seen.lock().unwrap().as_slice(), ["Fixture Town"]);

        let mut still = update(3);
        let mut still_input = known_input();
        let UiEvent::Update(still) = &mut still else {
            unreachable!();
        };
        still.metrics.points = (400, 240);
        still.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        still_input.pointer_position = Some(pointer);
        post_after_tick(&mut link, UiEvent::Update(still.clone()), still_input)
            .expect("still update posted");
        let _ = answer(&mut link);
        for _ in 0..5 {
            let _ = link.take_answer();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(seen.lock().unwrap().as_slice(), ["Fixture Town"]);

        let mut pan = update(4);
        let mut pan_input = known_input();
        let UiEvent::Update(pan) = &mut pan else {
            unreachable!();
        };
        pan.metrics.points = (400, 240);
        pan.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        pan_input.actions = vec![action("map_pan_right", true, false, false)];
        pan_input.pointer_position = Some(pointer);
        post_after_tick(&mut link, UiEvent::Update(pan.clone()), pan_input)
            .expect("pan update posted");
        let _ = answer(&mut link);
        wait_for_count(&mut link, 2);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["Fixture Town", "Fixture Town"]
        );

        let mut leave = update(5);
        let mut leave_input = known_input();
        let UiEvent::Update(leave) = &mut leave else {
            unreachable!();
        };
        leave.metrics.points = (400, 240);
        leave.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        leave_input.pointer_position = Some(PointerPosition {
            x: 401 * 256,
            y: 12 * 256,
        });
        post_after_tick(&mut link, UiEvent::Update(leave.clone()), leave_input)
            .expect("leave update posted");
        let _ = answer(&mut link);
        wait_for_count(&mut link, 3);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["Fixture Town", "Fixture Town", "nil"]
        );
    }

    #[test]
    fn game_select_delivers_the_latest_current_selection_once_on_registration() {
        let seen: Arc<std::sync::Mutex<Vec<BTreeMap<String, ConfigValue>>>> = Arc::default();
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                let Some(Some(ConfigValue::Table(fields))) = arguments.first() else {
                    return Err("game_select did not receive its selection table".to_owned());
                };
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(fields.clone());
                Ok(Vec::new())
            })
        };
        let time_registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let select_registered = Arc::new(AtomicBool::new(false));
        let harness = Harness::new(
            "game.on_select(function(selection) window.record(selection) end)",
            Extras {
                functions: vec![("record", record)],
                extensions: vec![game_api::extension_with_select(
                    Arc::clone(&time_registered),
                    Arc::clone(&hover_registered),
                    Arc::clone(&select_registered),
                    Arc::new(|| {}),
                    crate::game::controls::SimControls::new(std::sync::mpsc::channel().0),
                )],
                ..Extras::default()
            },
        );
        let mut controller = crate::map_app::tests::hover_controller();
        crate::map_app::tests::select_fixture(&mut controller);
        assert_eq!(
            controller
                .selection()
                .as_ref()
                .map(|selection| selection.seq),
            Some(1)
        );
        crate::map_app::tests::select_fixture(&mut controller);
        assert_eq!(
            controller
                .selection()
                .as_ref()
                .map(|selection| selection.seq),
            Some(2)
        );
        let map = crate::map_app::shared(controller);
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), map).with_sim(SimLink {
            views: Arc::default(),
            controls: crate::game::controls::SimControls::new(sender),
            slot: SimSlot::new(Arc::new(|| {})),
            time_registered,
            hover_registered,
            select_registered: Arc::clone(&select_registered),
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while !select_registered.load(Ordering::SeqCst) {
            let _ = link.take_answer();
            assert!(
                Instant::now() < deadline,
                "on_select registration was not observed"
            );
            thread::yield_now();
        }
        assert!(link.deliver_select());
        while seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            let _ = link.take_answer();
            assert!(Instant::now() < deadline, "game_select was not delivered");
            thread::yield_now();
        }
        let calls = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].get("seq"), Some(&ConfigValue::Integer(2)));
        assert_eq!(
            calls[0].get("id"),
            Some(&ConfigValue::String("fixture-town".to_owned()))
        );
        assert_eq!(
            calls[0].get("name"),
            Some(&ConfigValue::String("Fixture Town".to_owned()))
        );
        assert_eq!(
            calls[0].get("kind"),
            Some(&ConfigValue::String("test".to_owned()))
        );
        assert_eq!(calls[0].get("size"), Some(&ConfigValue::Integer(1)));
        assert!(
            !link.deliver_select(),
            "an unchanged sequence is not reposted"
        );
    }

    #[test]
    fn game_select_retries_after_full_and_posts_only_the_latest_selection() {
        let gate = Gate::new();
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            Arc::new(move |_| {
                gate.wait();
                Ok(Vec::new())
            })
        };
        let seen = Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                let Some(Some(ConfigValue::Table(fields))) = arguments.first() else {
                    return Err("game_select did not receive a sequence".to_owned());
                };
                let Some(ConfigValue::Integer(seq)) = fields.get("seq") else {
                    return Err("game_select did not receive an integer sequence".to_owned());
                };
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(*seq);
                Ok(Vec::new())
            })
        };
        let select_registered = Arc::new(AtomicBool::new(false));
        let time_registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let harness = Harness::new(
            "game.on_select(function(selection) window.record(selection) end)\nwindow.on_opened(function() window.hold() end)",
            Extras {
                functions: vec![("hold", hold), ("record", record)],
                extensions: vec![game_api::extension_with_select(
                    Arc::clone(&time_registered),
                    Arc::clone(&hover_registered),
                    Arc::clone(&select_registered),
                    Arc::new(|| {}),
                    crate::game::controls::SimControls::new(sender.clone()),
                )],
                ..Extras::default()
            },
        );
        let mut controller = crate::map_app::tests::hover_controller();
        crate::map_app::tests::select_fixture(&mut controller);
        crate::map_app::tests::select_fixture(&mut controller);
        let map = crate::map_app::shared(controller);
        let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), map).with_sim(SimLink {
            views: Arc::default(),
            controls: crate::game::controls::SimControls::new(sender),
            slot: SimSlot::new(Arc::new(|| {})),
            time_registered,
            hover_registered,
            select_registered: Arc::clone(&select_registered),
        });

        // Observe callback registration through the public post path, then fill all call places
        // with held callbacks. Selection seq 2 must be retained for a later retry.
        link.post(update(1)).expect("registration update posted");
        for _ in 0..UI_CALL_LIMIT - 1 {
            link.post(UiEvent::Opened)
                .expect("held lifecycle call fits");
        }
        assert_eq!(link.outstanding(), UI_CALL_LIMIT);
        assert!(
            !link.deliver_select(),
            "Full defers, but does not consume, selection"
        );
        gate.open();
        let deadline = Instant::now() + Duration::from_secs(10);
        while link.outstanding() != 0 {
            let _ = link.take_answer();
            assert!(Instant::now() < deadline, "held calls did not drain");
            thread::yield_now();
        }
        while seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            let _ = link.take_answer();
            assert!(
                Instant::now() < deadline,
                "retried game_select was not delivered"
            );
            thread::yield_now();
        }
        assert_eq!(
            *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [2],
            "only the latest sequence is delivered after Full"
        );
        assert!(
            !link.deliver_select(),
            "the delivered sequence is not duplicated"
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

    /// A paced update event (its input is the stash's, gathered by the ticks before it).
    fn update(sequence: u64) -> UiEvent {
        UiEvent::Update(UpdateEvent {
            sequence,
            elapsed: Duration::from_millis(500),
            dt: Duration::from_millis(250),
            metrics: known_metrics(),
        })
    }

    fn known_metrics() -> WindowMetrics {
        WindowMetrics {
            points: (100, 80),
            pixels: PixelSize {
                width: 200,
                height: 160,
            },
        }
    }

    /// A known snapshot: `move_left` held, `reload_texture` tapped, `move_up` unbound and untouched.
    fn known_input() -> InputSnapshot {
        InputSnapshot {
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
            typed: Vec::new(),
            typed_lost: false,
            typed_epoch: 0,
            capture: None,
            reset: false,
            overflow_count: 3,
        }
    }

    /// The tick that gathered `known_input()`.
    fn known_tick() -> InputTick {
        InputTick {
            dt: Duration::from_millis(250),
            metrics: known_metrics(),
            input: known_input(),
        }
    }

    fn hit_regions(keys: &[(&str, f64, f64, f64, f64)]) -> Arc<ck_render_2d::HitRegions> {
        let bridge = ck_render_2d::SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut writer = bridge.take_writer().expect("writer");
        for (key, x, y, width, height) in keys {
            writer
                .set_hit(
                    key,
                    Some(vec![ck_render_2d::PointRect::new(*x, *y, *width, *height)]),
                    None,
                )
                .expect("valid hit key");
        }
        assert!(writer.publish());
        bridge.hit_regions().expect("published regions")
    }

    fn pointer_input(
        events: Vec<PointerEvent>,
        position: Option<PointerPosition>,
        held: bool,
    ) -> InputSnapshot {
        let mut input = known_input();
        input.pointer_events = events;
        input.pointer_position = position;
        input.pointer_held = held;
        input
    }

    #[test]
    fn pointer_routing_captures_clicks_and_drags_and_keeps_map_drags_outside() {
        let regions = hit_regions(&[("panel", 0.0, 0.0, 20.0, 20.0)]);
        let inside = PointerPosition {
            x: 5 * 256,
            y: 5 * 256,
        };
        let outside = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let mut owner = GestureOwner::Idle;
        let click = route_pointer(
            &mut owner,
            &pointer_input(
                vec![PointerEvent::Down(inside), PointerEvent::Up(inside)],
                Some(inside),
                false,
            ),
            Some(&regions),
        );
        assert!(matches!(click.map.as_slice(), []));
        assert_eq!(
            click.ui.iter().map(|event| event.kind).collect::<Vec<_>>(),
            ["down", "up"]
        );
        assert!(
            click
                .ui
                .iter()
                .all(|event| event.key.as_deref() == Some("panel"))
        );
        assert_eq!(owner, GestureOwner::Idle);

        let drag = route_pointer(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(inside)], Some(outside), true),
            Some(&regions),
        );
        assert_eq!(drag.ui[0].kind, "down");
        let move_out = route_pointer(
            &mut owner,
            &pointer_input(
                vec![PointerEvent::Move(outside), PointerEvent::Up(outside)],
                Some(outside),
                false,
            ),
            Some(&regions),
        );
        assert_eq!(
            move_out
                .ui
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            ["move", "up"]
        );
        assert!(move_out.map.is_empty());

        let map_drag = route_pointer(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(outside)], Some(outside), true),
            Some(&regions),
        );
        assert!(matches!(
            map_drag.map.as_slice(),
            [crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Down(
                _
            ))]
        ));
        assert_eq!(map_drag.ui.len(), 1);
        assert_eq!(map_drag.ui[0].kind, "map_down");
        assert_eq!(map_drag.ui[0].key, None);
        assert_eq!(map_drag.ui[0].position, outside);
        assert!(!map_drag.ui[0].inside);
        let crossing = route_pointer(
            &mut owner,
            &pointer_input(vec![PointerEvent::Move(inside)], Some(inside), true),
            Some(&regions),
        );
        assert!(matches!(
            crossing.map.as_slice(),
            [crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Move(
                _
            ))]
        ));
        assert!(crossing.ui.is_empty());
    }

    #[test]
    fn middle_and_right_buttons_are_ui_events_without_claiming_a_gesture_or_reaching_the_map() {
        use ck_window::PointerButton;

        let regions = hit_regions(&[("panel", 0.0, 0.0, 20.0, 20.0)]);
        let inside = PointerPosition {
            x: 5 * 256,
            y: 5 * 256,
        };
        let outside = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let mut owner = GestureOwner::Idle;
        let routed = route_pointer(
            &mut owner,
            &pointer_input(
                vec![
                    PointerEvent::ButtonDown {
                        button: PointerButton::Middle,
                        position: inside,
                    },
                    PointerEvent::ButtonUp {
                        button: PointerButton::Middle,
                        position: inside,
                    },
                    PointerEvent::ButtonDown {
                        button: PointerButton::Right,
                        position: outside,
                    },
                    PointerEvent::ButtonUp {
                        button: PointerButton::Right,
                        position: outside,
                    },
                ],
                Some(outside),
                false,
            ),
            Some(&regions),
        );

        assert!(
            routed.map.is_empty(),
            "non-left buttons never reach map routing"
        );
        assert_eq!(
            owner,
            GestureOwner::Idle,
            "button edges do not claim a gesture"
        );
        assert_eq!(
            routed.ui,
            [
                crate::input_stash::PointerViewEvent {
                    kind: "button_down",
                    position: inside,
                    key: Some("panel".to_owned()),
                    inside: true,
                    button: Some("middle"),
                    amount: None,
                    unit: None,
                },
                crate::input_stash::PointerViewEvent {
                    kind: "button_up",
                    position: inside,
                    key: Some("panel".to_owned()),
                    inside: true,
                    button: Some("middle"),
                    amount: None,
                    unit: None,
                },
                crate::input_stash::PointerViewEvent {
                    kind: "button_down",
                    position: outside,
                    key: None,
                    inside: false,
                    button: Some("right"),
                    amount: None,
                    unit: None,
                },
                crate::input_stash::PointerViewEvent {
                    kind: "button_up",
                    position: outside,
                    key: None,
                    inside: false,
                    button: Some("right"),
                    amount: None,
                    unit: None,
                },
            ]
        );
    }

    #[test]
    fn non_left_buttons_leave_ui_capture_and_map_drag_ownership_untouched() {
        use ck_window::PointerButton;

        let regions = hit_regions(&[("panel", 0.0, 0.0, 20.0, 20.0)]);
        let inside = PointerPosition {
            x: 5 * 256,
            y: 5 * 256,
        };
        let outside = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let mut owner = GestureOwner::Ui("panel".to_owned());
        let ui_capture = route_pointer(
            &mut owner,
            &pointer_input(
                vec![
                    PointerEvent::ButtonDown {
                        button: PointerButton::Middle,
                        position: inside,
                    },
                    PointerEvent::Up(inside),
                ],
                Some(inside),
                false,
            ),
            Some(&regions),
        );
        assert!(ui_capture.map.is_empty());
        assert_eq!(
            ui_capture
                .ui
                .iter()
                .map(|event| (event.kind, event.key.as_deref()))
                .collect::<Vec<_>>(),
            [("button_down", Some("panel")), ("up", Some("panel"))]
        );
        assert_eq!(owner, GestureOwner::Idle);

        let mut owner = GestureOwner::Map;
        let map_drag = route_pointer(
            &mut owner,
            &pointer_input(
                vec![
                    PointerEvent::ButtonDown {
                        button: PointerButton::Right,
                        position: inside,
                    },
                    PointerEvent::ButtonUp {
                        button: PointerButton::Right,
                        position: inside,
                    },
                ],
                Some(inside),
                true,
            ),
            Some(&regions),
        );
        assert!(map_drag.map.is_empty());
        assert_eq!(
            owner,
            GestureOwner::Map,
            "middle/right edges neither end nor replace map ownership"
        );

        let left_release = route_pointer(
            &mut owner,
            &pointer_input(vec![PointerEvent::Up(outside)], Some(outside), false),
            Some(&regions),
        );
        assert!(matches!(
            left_release.map.as_slice(),
            [crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Up(position))] if *position == outside
        ));
        assert_eq!(owner, GestureOwner::Idle);
    }

    #[test]
    fn ending_a_capture_ends_a_map_drag_and_cancels_a_widget_gesture_and_the_rest_reaches_nobody() {
        let regions = hit_regions(&[("backdrop", 0.0, 0.0, 400.0, 300.0)]);
        let at = |x: i32| PointerPosition {
            x: x * 256,
            y: 50 * 256,
        };
        // Control: a map drag goes on (the backdrop alone does not stop a drag the map already owns).
        let mut owner = GestureOwner::Map;
        let kept = route_pointer_with(
            &mut owner,
            &pointer_input(vec![PointerEvent::Move(at(60))], Some(at(60)), true),
            Some(&regions),
            None,
            false,
        );
        assert!(matches!(
            kept.map.as_slice(),
            [crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Move(
                _
            ))]
        ));
        assert_eq!(owner, GestureOwner::Map);

        // The tick the menu's block turns on: the map's drag ends, and the move that tick carried goes nowhere.
        let ended = route_pointer_with(
            &mut owner,
            &pointer_input(vec![PointerEvent::Move(at(80))], Some(at(80)), true),
            Some(&regions),
            None,
            true,
        );
        assert!(matches!(
            ended.map.as_slice(),
            [crate::map_app::RoutedMapEvent::EndDrag]
        ));
        assert!(ended.ui.is_empty());
        assert_eq!(owner, GestureOwner::Nobody);
        // The held button's later moves and its release reach neither the map nor the UI; then the pointer is idle again.
        for (events, held) in [
            (vec![PointerEvent::Move(at(100))], true),
            (vec![PointerEvent::Up(at(100))], false),
        ] {
            let routed = route_pointer_with(
                &mut owner,
                &pointer_input(events, Some(at(100)), held),
                Some(&regions),
                None,
                false,
            );
            assert!(routed.map.is_empty() && routed.ui.is_empty());
        }
        assert_eq!(owner, GestureOwner::Idle);

        // A widget's gesture is cancelled, with its key, instead.
        let mut owner = GestureOwner::Ui("backdrop".to_owned());
        let cancelled = route_pointer_with(
            &mut owner,
            &pointer_input(Vec::new(), Some(at(80)), true),
            Some(&regions),
            None,
            true,
        );
        assert!(cancelled.map.is_empty());
        assert_eq!(cancelled.ui.len(), 1);
        assert_eq!(cancelled.ui[0].kind, "cancel");
        assert_eq!(cancelled.ui[0].key.as_deref(), Some("backdrop"));
        assert_eq!(owner, GestureOwner::Nobody);

        // With nothing captured, ending a capture changes nothing, and a press on the backdrop is the UI's, not the map's.
        let mut owner = GestureOwner::Idle;
        let pressed = route_pointer_with(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(at(80))], Some(at(80)), true),
            Some(&regions),
            None,
            true,
        );
        assert!(pressed.map.is_empty());
        assert_eq!(pressed.ui.len(), 1);
        assert_eq!(pressed.ui[0].kind, "down");
        assert_eq!(pressed.ui[0].key.as_deref(), Some("backdrop"));
    }

    #[test]
    fn non_left_button_positions_are_converted_to_ui_units_in_pixel_windows() {
        use ck_render_2d::ui_scale::UnitMap;
        use ck_window::PointerButton;

        let units = Some(UnitMap {
            pixels: PixelSize {
                width: 1280,
                height: 800,
            },
            points: (640, 400),
            k: 2,
        });
        let point_position = PointerPosition {
            x: 10 * 256,
            y: 7 * 256,
        };
        let regions = hit_regions(&[("panel", 19.0, 13.0, 2.0, 2.0)]);
        let mut owner = GestureOwner::Idle;
        let routed = route_pointer_in(
            &mut owner,
            &pointer_input(
                vec![PointerEvent::ButtonDown {
                    button: PointerButton::Middle,
                    position: point_position,
                }],
                Some(point_position),
                false,
            ),
            Some(&regions),
            units,
        );
        assert!(routed.map.is_empty());
        assert_eq!(routed.ui.len(), 1);
        assert_eq!(
            routed.ui[0].position,
            PointerPosition {
                x: 20 * 256,
                y: 14 * 256
            }
        );
        assert_eq!(routed.ui[0].key.as_deref(), Some("panel"));
        assert_eq!(routed.ui[0].button, Some("middle"));
        assert!(routed.ui[0].inside);
    }

    #[test]
    fn in_a_window_opened_in_pixels_the_ui_gets_units_and_the_map_keeps_points() {
        use ck_render_2d::ui_scale::UnitMap;
        // The scale 1.5 on a 2x display: 1920x1200 pixels, 960x600 points.
        let units = Some(UnitMap {
            pixels: PixelSize {
                width: 1920,
                height: 1200,
            },
            points: (960, 600),
            k: 3,
        });
        // A region at units (130, 60) sized 4 by 7.
        let regions = hit_regions(&[("probe", 130.0, 60.0, 4.0, 7.0)]);
        let inside = PointerPosition {
            x: 100 * 256,
            y: 50 * 256,
        };
        // 100.5 points is unit 134.0 exactly: on the half-open right edge.
        let edge = PointerPosition {
            x: 100 * 256 + 128,
            y: 50 * 256,
        };
        let mut owner = GestureOwner::Idle;
        let down = route_pointer_in(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(inside)], Some(inside), true),
            Some(&regions),
            units,
        );
        assert_eq!(down.ui.len(), 1, "inside the region in units");
        assert_eq!(down.ui[0].key.as_deref(), Some("probe"));
        assert_eq!(
            down.ui[0].position,
            PointerPosition { x: 34133, y: 17066 },
            "133.33203125 and 66.6640625 units"
        );
        assert!(down.map.is_empty(), "a UI gesture is not the map's");
        let mut owner = GestureOwner::Idle;
        let outside = route_pointer_in(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(edge)], Some(edge), true),
            Some(&regions),
            units,
        );
        assert_eq!(outside.ui.len(), 1, "the map press is reported to the UI");
        assert_eq!(outside.ui[0].kind, "map_down");
        assert_eq!(
            outside.ui[0].position,
            PointerPosition {
                x: 134 * 256,
                y: 17_066
            }
        );
        // The map's event keeps its position in points.
        assert!(matches!(
            outside.map.as_slice(),
            [crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Down(position))] if *position == edge
        ));
        // Point scrolls are converted toward zero, line scrolls are not.
        let mut owner = GestureOwner::Idle;
        let scrolled = route_pointer_in(
            &mut owner,
            &pointer_input(
                vec![
                    PointerEvent::Scroll {
                        position: inside,
                        amount: 15 * 256,
                        unit: ScrollUnit::Points,
                    },
                    PointerEvent::Scroll {
                        position: inside,
                        amount: -256,
                        unit: ScrollUnit::Points,
                    },
                    PointerEvent::Scroll {
                        position: inside,
                        amount: 256,
                        unit: ScrollUnit::Lines,
                    },
                ],
                Some(inside),
                false,
            ),
            Some(&regions),
            units,
        );
        assert_eq!(
            scrolled
                .ui
                .iter()
                .map(|event| event.amount)
                .collect::<Vec<_>>(),
            [Some(20 * 256), Some(-341), Some(256)]
        );
    }

    #[test]
    fn an_update_in_a_window_opened_in_pixels_carries_the_view_in_units_and_the_drawable_in_pixels()
    {
        let UiEvent::Update(event) = update(3) else {
            unreachable!();
        };
        let stash = InputStash::default();
        let fields = |view: Option<(u32, u32)>| {
            let ConfigValue::Table(fields) = update_table_in(&event, &stash, view).expect("table")
            else {
                panic!("a table");
            };
            fields
        };
        // The window's points and pixels are the same metrics' either way; `width` and `height` are the view the UI lays out in.
        let points = fields(None);
        assert_eq!(points["width"], integer(event.metrics.points.0));
        let units = fields(Some((1280, 800)));
        assert_eq!(units["width"], ConfigValue::Integer(1280));
        assert_eq!(units["height"], ConfigValue::Integer(800));
        assert_eq!(units["pixel_width"], integer(event.metrics.pixels.width));
        assert_eq!(units["pixel_height"], integer(event.metrics.pixels.height));
    }

    /// Through delivery: the same raw hover, delivered at two conversions (the display goes from 2 pixels a point to 1 with the points
    /// unchanged), reaches `game.on_hover` twice, the second time with the new coordinates.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one end-to-end witness: a pixels window's hover, the link, two deliveries"
    )]
    fn the_same_hover_reaches_the_ui_again_with_new_coordinates_when_the_conversion_changes() {
        struct Null;
        impl ck_render_2d::QuadSink for Null {
            fn publish(&self, snapshot: ck_render_2d::Snapshot) -> Option<ck_render_2d::Snapshot> {
                Some(snapshot)
            }
        }
        let seen: Arc<std::sync::Mutex<Vec<f64>>> = Arc::default();
        let record: HostFunction = {
            let seen = Arc::clone(&seen);
            Arc::new(move |arguments| {
                let left = match arguments.first().and_then(Option::as_ref) {
                    Some(ConfigValue::Table(fields)) => match fields.get("left") {
                        Some(ConfigValue::Float(left)) => *left,
                        other => panic!("a hover left that is not a number: {other:?}"),
                    },
                    _ => f64::NAN,
                };
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(left);
                Ok(Vec::new())
            })
        };
        let time_registered = Arc::new(AtomicBool::new(false));
        let hover_registered = Arc::new(AtomicBool::new(false));
        let harness = Harness::new(
            "game.on_hover(function(h) window.record(h) end)\nframe.on_update(function() end)",
            Extras {
                functions: vec![("record", record)],
                extensions: vec![
                    frame_extension(),
                    game_api::extension(
                        Arc::clone(&time_registered),
                        Arc::clone(&hover_registered),
                        Arc::new(|| {}),
                        crate::game::controls::SimControls::new(std::sync::mpsc::channel().0),
                    ),
                ],
                ..Extras::default()
            },
        );
        // A window opened in pixels, 400x240 points at 2 pixels a point (800x480 pixels, the scale 1: a unit is half a point).
        harness
            .bridge
            .attach(
                &ck_window::WindowDesc {
                    title: "t".to_owned(),
                    width: 400,
                    height: 240,
                    pixel_request: Some(ck_window::PixelRequest {
                        size: (1280, 800),
                        min: Some((1280, 800)),
                    }),
                    ..ck_window::WindowDesc::default()
                },
                PixelSize {
                    width: 800,
                    height: 480,
                },
                Arc::new(Null),
            )
            .expect("attached");
        let map = crate::map_app::shared(crate::map_app::tests::window_case_hover_controller());
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mut link = CodekarlUi::with_map(harness.ui.slot.clone(), Arc::clone(&map))
            .with_scene_bridge(Arc::clone(&harness.bridge))
            .with_sim(SimLink {
                views: Arc::default(),
                controls: crate::game::controls::SimControls::new(sender),
                slot,
                time_registered,
                hover_registered,
                select_registered: Arc::new(AtomicBool::new(false)),
            });
        let pointer = PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let tick_at = |pixels: (u32, u32)| {
            let mut tick = known_tick();
            tick.metrics.points = (400, 240);
            tick.metrics.pixels = PixelSize {
                width: pixels.0,
                height: pixels.1,
            };
            tick.input.pointer_position = Some(pointer);
            tick.input.pointer_events = Vec::new();
            tick
        };
        // Put the hover on the marker (as the existing hover cases do), then let the link see the pointer so that it is the map's latest.
        let mut set_up = tick_at((800, 480));
        set_up.input.pointer_events = vec![
            PointerEvent::Scroll {
                position: pointer,
                amount: 15 * 256,
                unit: ScrollUnit::Points,
            },
            PointerEvent::Move(pointer),
        ];
        crate::map_app::lock(&map)
            .update(&set_up)
            .expect("the controller replay succeeds");
        let raw = crate::map_app::lock(&map)
            .hover()
            .expect("the pointer is over the fixture town");
        let wait_for = |link: &mut CodekarlUi, count: usize| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
                < count
                || link.outstanding() > 0
            {
                let _ = link.take_answer();
                assert!(Instant::now() < deadline, "delivery {count} never finished");
                thread::yield_now();
            }
        };
        // The first conversion: 800 pixels over 400 points at the scale 1 doubles a coordinate.
        link.input(&tick_at((800, 480))).expect("a tick");
        link.deliver_hover();
        wait_for(&mut link, 1);
        let first = *seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .expect("a delivery");
        assert!((first - f64::from(2 * i32::try_from(raw.left).expect("small"))).abs() < 0.01);
        let delivered = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        // The display goes to 1 pixel a point: the same raw hover, a conversion of 1 to 1.
        harness.bridge.set_metrics(
            (400, 240),
            PixelSize {
                width: 400,
                height: 240,
            },
        );
        link.input(&tick_at((400, 240))).expect("a tick");
        link.deliver_hover();
        wait_for(&mut link, delivered + 1);
        let values = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(values.len(), delivered + 1, "one more delivery: {values:?}");
        let second = *values.last().expect("a delivery");
        assert!(
            (second - f64::from(i32::try_from(raw.left).expect("small"))).abs() < 0.01,
            "the same raw hover (left {}) now converts to {second}, not {first}",
            raw.left
        );
    }

    #[test]
    fn a_hover_is_delivered_again_when_only_the_conversion_changes() {
        use ck_render_2d::ui_scale::UnitMap;
        let hover = Some(crate::map_app::Hover {
            location_index: 0,
            id: "town".to_owned(),
            name: "Town".to_owned(),
            left: 100,
            right: 160,
            y: 50,
        });
        // The points stay 960x600 and the display goes from 2 pixels a point to 1: the scale falls from 1.5 (k 3) to 1 (k 2), and the
        // same raw bounds are 133.33 units at one and 100 at the other.
        let at = |pixels: (u32, u32), k: u32| {
            Some(UnitMap {
                pixels: PixelSize {
                    width: pixels.0,
                    height: pixels.1,
                },
                points: (960, 600),
                k,
            })
        };
        let (before, after) = (at((1920, 1200), 3), at((960, 600), 2));
        let delivered = DeliveredHover::Sent(hover.clone(), before);
        assert!(
            delivered.is_current(hover.as_ref(), before),
            "nothing changed"
        );
        assert!(
            !delivered.is_current(hover.as_ref(), after),
            "the converted bounds changed, so they are posted again"
        );
        assert!(!delivered.is_current(None, before), "a different hover");
        // No hover has nothing converted: a change of conversion alone posts nothing.
        let none = DeliveredHover::Sent(None, before);
        assert!(none.is_current(None, after));
        assert!(!DeliveredHover::Unsent.is_current(None, after));
        // A window opened in points has no conversion at all.
        let points = DeliveredHover::Sent(hover.clone(), None);
        assert!(points.is_current(hover.as_ref(), None));
    }

    #[test]
    fn the_hover_the_map_hands_the_ui_is_in_units_in_a_window_opened_in_pixels() {
        use ck_render_2d::ui_scale::UnitMap;
        let hover = crate::map_app::Hover {
            location_index: 0,
            id: "town".to_owned(),
            name: "Town".to_owned(),
            left: 100,
            right: 160,
            y: 50,
        };
        let value = |arguments: &[ConfigValue], key: &str| match &arguments[0] {
            ConfigValue::Table(fields) => fields[key].clone(),
            other => panic!("{other:?}"),
        };
        let points = game_api::hover_arguments(Some(&hover), None);
        assert_eq!(value(&points, "left"), ConfigValue::Integer(100));
        let units = Some(UnitMap {
            pixels: PixelSize {
                width: 1920,
                height: 1200,
            },
            points: (960, 600),
            k: 3,
        });
        let converted = game_api::hover_arguments(Some(&hover), units);
        // 100 points are 34133/256 units (133.33203125), 160 are 54613/256 (213.33203125) and 50 are 17066/256 (66.6640625), as numbers: the
        // pointer's conversion at its 1/256 precision.
        assert_eq!(
            value(&converted, "left"),
            ConfigValue::Float(34133.0 / 256.0)
        );
        assert_eq!(
            value(&converted, "right"),
            ConfigValue::Float(54613.0 / 256.0)
        );
        assert_eq!(value(&converted, "y"), ConfigValue::Float(17066.0 / 256.0));
    }

    #[test]
    fn overlap_scroll_cancel_overflow_and_lost_down_follow_the_router_owner() {
        let regions = hit_regions(&[
            ("lower", 0.0, 0.0, 30.0, 30.0),
            ("upper", 5.0, 5.0, 20.0, 20.0),
        ]);
        let inside = PointerPosition {
            x: 10 * 256,
            y: 10 * 256,
        };
        let mut owner = GestureOwner::Idle;
        let top = route_pointer(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(inside)], Some(inside), true),
            Some(&regions),
        );
        assert_eq!(top.ui[0].key.as_deref(), Some("upper"));
        let scroll = route_pointer(
            &mut owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: inside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(inside),
                true,
            ),
            Some(&regions),
        );
        assert_eq!(scroll.ui[0].kind, "scroll");
        assert_eq!(scroll.ui[0].key.as_deref(), Some("upper"));
        let overflow = route_pointer(
            &mut owner,
            &{
                let mut input = pointer_input(Vec::new(), Some(inside), true);
                input.pointer_overflow = true;
                input
            },
            Some(&regions),
        );
        assert_eq!(overflow.ui.len(), 1);
        assert_eq!(overflow.ui[0].kind, "move");

        let removed = hit_regions(&[("lower", 0.0, 0.0, 30.0, 30.0)]);
        let cancel = route_pointer(
            &mut owner,
            &pointer_input(
                vec![PointerEvent::Move(inside), PointerEvent::Up(inside)],
                Some(inside),
                false,
            ),
            Some(&removed),
        );
        assert_eq!(
            cancel.ui.iter().map(|event| event.kind).collect::<Vec<_>>(),
            ["cancel"]
        );
        assert!(cancel.map.is_empty());
        assert_eq!(owner, GestureOwner::Idle, "the up snapshot ends Nobody");
        let after_cancel = route_pointer(
            &mut owner,
            &pointer_input(
                vec![PointerEvent::Move(inside), PointerEvent::Up(inside)],
                Some(inside),
                false,
            ),
            Some(&regions),
        );
        assert_eq!(
            after_cancel
                .ui
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            ["move"]
        );
        assert!(after_cancel.map.is_empty());
        assert_eq!(owner, GestureOwner::Idle);
        let next_gesture = route_pointer(
            &mut owner,
            &pointer_input(vec![PointerEvent::Down(inside)], Some(inside), true),
            Some(&regions),
        );
        assert_eq!(next_gesture.ui[0].kind, "down");
        assert_eq!(next_gesture.ui[0].key.as_deref(), Some("upper"));

        let mut lost = GestureOwner::Idle;
        let no_down = route_pointer(
            &mut lost,
            &pointer_input(Vec::new(), Some(inside), true),
            Some(&regions),
        );
        assert!(no_down.map.is_empty() && no_down.ui.is_empty());
        assert_eq!(lost, GestureOwner::Nobody);
        let lost_move = route_pointer(
            &mut lost,
            &pointer_input(vec![PointerEvent::Move(inside)], Some(inside), true),
            Some(&regions),
        );
        assert!(lost_move.map.is_empty() && lost_move.ui.is_empty());
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one ordered sequence covers reset, overflow cancellation, release, and the next routed input"
    )]
    fn reset_and_overflow_cancel_captures_but_idle_and_released_nobody_route_again() {
        use crate::map_app::RoutedMapEvent as MapEvent;
        let regions = hit_regions(&[("panel", 0.0, 0.0, 20.0, 20.0)]);
        let inside = PointerPosition {
            x: 5 * 256,
            y: 5 * 256,
        };
        let outside = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };

        let mut idle_owner = GestureOwner::Idle;
        let mut reset_idle = pointer_input(Vec::new(), Some(outside), false);
        reset_idle.reset = true;
        let reset = route_pointer(&mut idle_owner, &reset_idle, Some(&regions));
        assert!(reset.ui.is_empty() && reset.map.is_empty());
        assert_eq!(idle_owner, GestureOwner::Idle);
        let after_reset_map_scroll = route_pointer(
            &mut idle_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: outside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(outside),
                false,
            ),
            Some(&regions),
        );
        assert!(after_reset_map_scroll.ui.is_empty());
        assert!(matches!(
            after_reset_map_scroll.map.as_slice(),
            [MapEvent::Pointer(PointerEvent::Scroll {
                position,
                amount: 256,
                unit: ScrollUnit::Lines,
            })] if *position == outside
        ));
        let after_reset_ui_scroll = route_pointer(
            &mut idle_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: inside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(inside),
                false,
            ),
            Some(&regions),
        );
        assert!(after_reset_ui_scroll.map.is_empty());
        assert_eq!(after_reset_ui_scroll.ui[0].key.as_deref(), Some("panel"));

        let mut ui_owner = GestureOwner::Ui("panel".to_owned());
        let mut reset_capture = pointer_input(
            vec![PointerEvent::Move(outside), PointerEvent::Up(outside)],
            Some(outside),
            true,
        );
        reset_capture.reset = true;
        let cancelled = route_pointer(&mut ui_owner, &reset_capture, Some(&regions));
        assert_eq!(
            cancelled
                .ui
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            ["cancel"]
        );
        assert!(cancelled.map.is_empty());
        assert_eq!(ui_owner, GestureOwner::Nobody);

        let released = pointer_input(Vec::new(), Some(outside), false);
        let resumed = route_pointer(&mut ui_owner, &released, Some(&regions));
        assert!(resumed.ui.is_empty() && resumed.map.is_empty());
        assert_eq!(ui_owner, GestureOwner::Idle);
        let resumed_scroll = route_pointer(
            &mut ui_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: outside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(outside),
                false,
            ),
            Some(&regions),
        );
        assert_eq!(resumed_scroll.map.len(), 1);

        let mut overflow_owner = GestureOwner::Ui("panel".to_owned());
        let mut overflow_up = pointer_input(Vec::new(), Some(inside), false);
        overflow_up.pointer_overflow = true;
        let overflow_cancel = route_pointer(&mut overflow_owner, &overflow_up, Some(&regions));
        assert_eq!(
            overflow_cancel
                .ui
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            ["cancel"]
        );
        assert_eq!(overflow_owner, GestureOwner::Idle);
    }

    #[test]
    fn reset_cancels_map_drag_and_scroll_routes_by_position_except_during_ui_capture() {
        use crate::map_app::RoutedMapEvent as MapEvent;
        let regions = hit_regions(&[("panel", 0.0, 0.0, 20.0, 20.0)]);
        let inside = PointerPosition {
            x: 5 * 256,
            y: 5 * 256,
        };
        let outside = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };

        let mut map_owner = GestureOwner::Map;
        let mut reset_map = pointer_input(Vec::new(), Some(outside), true);
        reset_map.reset = true;
        assert!(matches!(
            route_pointer(&mut map_owner, &reset_map, Some(&regions))
                .map
                .as_slice(),
            [MapEvent::EndDrag]
        ));
        assert_eq!(map_owner, GestureOwner::Nobody);

        let mut idle_owner = GestureOwner::Idle;
        let scroll_in = route_pointer(
            &mut idle_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: inside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(inside),
                false,
            ),
            Some(&regions),
        );
        assert_eq!(scroll_in.ui[0].kind, "scroll");
        assert_eq!(scroll_in.ui[0].key.as_deref(), Some("panel"));

        let mut map_owner = GestureOwner::Map;
        let map_scroll_in = route_pointer(
            &mut map_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: inside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(inside),
                true,
            ),
            Some(&regions),
        );
        assert_eq!(map_scroll_in.ui[0].key.as_deref(), Some("panel"));
        assert!(map_scroll_in.map.is_empty());
        assert_eq!(map_owner, GestureOwner::Map);

        let map_scroll_out = route_pointer(
            &mut map_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: outside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(outside),
                true,
            ),
            Some(&regions),
        );
        assert!(matches!(
            map_scroll_out.map.as_slice(),
            [MapEvent::Pointer(PointerEvent::Scroll {
                position,
                amount: 256,
                unit: ScrollUnit::Lines
            })] if *position == outside
        ));

        let mut ui_owner = GestureOwner::Ui("panel".to_owned());
        let captured_scroll_out = route_pointer(
            &mut ui_owner,
            &pointer_input(
                vec![PointerEvent::Scroll {
                    position: outside,
                    amount: 256,
                    unit: ScrollUnit::Lines,
                }],
                Some(outside),
                true,
            ),
            Some(&regions),
        );
        assert_eq!(captured_scroll_out.ui[0].key.as_deref(), Some("panel"));
        assert!(captured_scroll_out.map.is_empty());
    }

    #[test]
    fn a_new_ui_down_ends_a_map_drag_at_its_position_before_overflow_resync() {
        use crate::map_app::RoutedMapEvent::EndDrag;
        let regions = hit_regions(&[("panel", 0.0, 0.0, 20.0, 20.0)]);
        let outside = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let inside = PointerPosition {
            x: 5 * 256,
            y: 5 * 256,
        };
        let mut owner = GestureOwner::Map;
        let mut input = pointer_input(vec![PointerEvent::Down(inside)], Some(inside), true);
        input.pointer_overflow = true;
        let routed = route_pointer(&mut owner, &input, Some(&regions));
        assert!(matches!(routed.map.as_slice(), [EndDrag]));
        assert_eq!(routed.ui[0].kind, "down");
        assert_eq!(owner, GestureOwner::Ui("panel".to_owned()));

        let mut map_owner = GestureOwner::Map;
        let moved = route_pointer(
            &mut map_owner,
            &pointer_input(
                vec![PointerEvent::Down(outside), PointerEvent::Move(inside)],
                Some(inside),
                true,
            ),
            Some(&regions),
        );
        assert!(matches!(
            moved.map.as_slice(),
            [
                EndDrag,
                crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Down(_)),
                crate::map_app::RoutedMapEvent::Pointer(PointerEvent::Move(_))
            ]
        ));
        assert_eq!(moved.ui.len(), 1);
        assert_eq!(moved.ui[0].kind, "map_down");
        assert_eq!(moved.ui[0].position, outside);
    }

    /// The stash after one tick of `input`.
    fn stash_of(input: &InputSnapshot) -> InputStash {
        let mut stash = InputStash::default();
        stash.absorb(input);
        stash
    }

    /// What the loop does: the input tick of `input` first, then the update that is offered to the UI.
    fn post_after_tick(
        link: &mut CodekarlUi,
        event: UiEvent,
        input: InputSnapshot,
    ) -> Result<(), PostError> {
        let UiEvent::Update(update) = &event else {
            unreachable!("an update");
        };
        link.input(&InputTick {
            dt: update.dt,
            metrics: update.metrics,
            input,
        })
        .expect("the tick is consumed");
        link.post(event)
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
            ("typed".to_owned(), ConfigValue::Array(Vec::new())),
            ("typed_lost".to_owned(), ConfigValue::Bool(false)),
            ("typed_epoch".to_owned(), ConfigValue::Integer(0)),
        ]))
    }

    #[test]
    fn frame_update_pointer_has_literal_point_units_keys_and_overflow() {
        let position = PointerPosition { x: 384, y: -128 };
        let mut stash = InputStash::default();
        stash.absorb_pointer(
            Some(crate::input_stash::PointerView {
                position,
                held: true,
                over: Some("readout".to_owned()),
            }),
            [
                crate::input_stash::PointerViewEvent {
                    kind: "down",
                    position,
                    key: Some("readout".to_owned()),
                    inside: true,
                    button: None,
                    amount: None,
                    unit: None,
                },
                crate::input_stash::PointerViewEvent {
                    kind: "scroll",
                    position,
                    key: Some("readout".to_owned()),
                    inside: false,
                    button: None,
                    amount: Some(-128),
                    unit: Some("lines"),
                },
                crate::input_stash::PointerViewEvent {
                    kind: "button_up",
                    position,
                    key: Some("readout".to_owned()),
                    inside: true,
                    button: Some("right"),
                    amount: None,
                    unit: None,
                },
            ],
            true,
        );
        let UiEvent::Update(update_event) = update(31) else {
            unreachable!();
        };
        let ConfigValue::Table(fields) = update_table(&update_event, &stash).expect("table") else {
            panic!("update is a table");
        };
        let ConfigValue::Table(pointer) = &fields["pointer"] else {
            panic!("pointer is present");
        };
        assert_eq!(pointer["x"], ConfigValue::Float(1.5));
        assert_eq!(pointer["y"], ConfigValue::Float(-0.5));
        assert_eq!(pointer["held"], ConfigValue::Bool(true));
        assert_eq!(pointer["over"], ConfigValue::String("readout".to_owned()));
        assert_eq!(pointer["overflow"], ConfigValue::Bool(true));
        let ConfigValue::Array(events) = &pointer["events"] else {
            panic!("pointer events are an array");
        };
        assert_eq!(events.len(), 3);
        let ConfigValue::Table(button) = &events[2] else {
            panic!("button event is a table");
        };
        assert_eq!(button["kind"], ConfigValue::String("button_up".to_owned()));
        assert_eq!(button["button"], ConfigValue::String("right".to_owned()));
        assert!(!button.contains_key("amount") && !button.contains_key("unit"));
        let ConfigValue::Table(scroll) = &events[1] else {
            panic!("scroll event is a table");
        };
        assert_eq!(scroll["kind"], ConfigValue::String("scroll".to_owned()));
        assert_eq!(scroll["x"], ConfigValue::Float(1.5));
        assert_eq!(scroll["y"], ConfigValue::Float(-0.5));
        assert_eq!(scroll["key"], ConfigValue::String("readout".to_owned()));
        assert_eq!(scroll["amount"], ConfigValue::Float(-0.5));
        assert_eq!(scroll["unit"], ConfigValue::String("lines".to_owned()));
        // Every event says whether it was inside its key's published rectangles.
        let ConfigValue::Table(down) = &events[0] else {
            panic!("down event is a table");
        };
        assert_eq!(down["inside"], ConfigValue::Bool(true));
        assert_eq!(scroll["inside"], ConfigValue::Bool(false));
    }

    #[test]
    fn an_update_is_the_frame_update_call_with_the_table_of_the_design() {
        // Negative control: a wrong name, a missing key, an unbound action left out or a transition's kind swapped
        // changes this exact value.
        assert_eq!(
            call_of(&update(5), &stash_of(&known_input())),
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
            call_of(&update(top), &stash_of(&known_input())),
            Ok(("frame_update", vec![expected_table(i64::MAX)]))
        );
        let message = call_of(&update(top + 1), &InputStash::default())
            .expect_err("2^63 is not a Lua integer");
        assert!(message.contains(&(top + 1).to_string()), "{message}");
        assert!(message.contains("Lua integer"), "{message}");
        assert!(call_of(&update(u64::MAX), &InputStash::default()).is_err());
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
        // A stash of an input with no actions and no transitions.
        let mut input = known_input();
        input.actions.clear();
        input.transitions.clear();
        input.overflow = false;
        let (_, arguments) = call_of(&update(1), &stash_of(&input)).expect("a call");
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
        post_after_tick(&mut link, update(9), known_input()).expect("posted");
        let got = answer(&mut link);
        assert_eq!(got.answer, Answer::Returned);
        assert_eq!(got.event, update(9));
        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A real VM reads an empty list back as an empty table: the update's empty `typed` list (CK-314) is one.
        let mut expected = expected_table(9);
        if let ConfigValue::Table(fields) = &mut expected {
            fields.insert("typed".to_owned(), ConfigValue::Table(BTreeMap::new()));
        }
        assert_eq!(
            &*seen,
            &[expected],
            "the script read exactly the table of the design (a held and a tapped action, an unbound action present and false, the transitions in order, overflow and reset)"
        );
        assert_eq!(link.outstanding(), 0);
    }

    #[test]
    fn the_update_carries_the_typed_entries_as_one_ordered_list_with_the_loss_and_the_epoch() {
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
        // The script reads the three fields back as the strings a widget would see.
        let harness = frame_harness(
            "local parts = {}\n\
             for _, e in ipairs(f.typed) do parts[#parts + 1] = e.kind .. (e.text ~= nil and (':' .. e.text) or '') end\n\
             window.record(table.concat(parts, ',') .. '|' .. tostring(f.typed_lost) .. '|' .. f.typed_epoch)",
            vec![("record", record)],
        );
        let mut link = harness.ui.clone();
        let mut input = known_input();
        input.typed = vec![
            ck_window::TypedEntry::Char('a'),
            ck_window::TypedEntry::Char('\u{e1}'),
            ck_window::TypedEntry::Backspace,
            ck_window::TypedEntry::Enter,
            ck_window::TypedEntry::Tab,
            ck_window::TypedEntry::Left,
            ck_window::TypedEntry::Right,
        ];
        input.typed_lost = true;
        input.typed_epoch = 5;
        post_after_tick(&mut link, update(3), input).expect("posted");
        assert_eq!(answer(&mut link).answer, Answer::Returned);
        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            &*seen,
            &[ConfigValue::String(
                "char:a,char:\u{e1},backspace,enter,tab,left,right|true|5".to_owned()
            )]
        );
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
            economy_revision: 0,
            economy: None,
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
        let views = Arc::new(game_api::ViewCache::default());
        let mut extras = extras;
        extras.functions.push(("record", record));
        extras.extensions.push(
            game_api::extension(
                Arc::clone(&registered),
                Arc::new(AtomicBool::new(false)),
                Arc::new(|| {}),
                crate::game::controls::SimControls::new(std::sync::mpsc::channel().0),
            )
            .function("place", game_api::place_function(Arc::clone(&views))),
        );
        let harness = Harness::new(script, extras);
        let slot = SimSlot::new(Arc::new(|| {}));
        let (sender, _receiver) = std::sync::mpsc::channel();
        let link = harness.ui.clone().with_sim(SimLink {
            views,
            controls: crate::game::controls::SimControls::new(sender),
            slot: Arc::clone(&slot),
            time_registered: registered,
            hover_registered: Arc::new(AtomicBool::new(false)),
            select_registered: Arc::new(AtomicBool::new(false)),
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

    /// A snapshot that carries the economy view `view` under `revision`.
    fn snapshot_with_view(
        hour: u8,
        revision: u64,
        view: Arc<crate::game::view::EconomyView>,
    ) -> SimSnapshot {
        let mut snapshot = snapshot_at(hour, 1, None);
        snapshot.economy_revision = revision;
        snapshot.economy = Some(view);
        snapshot
    }

    /// A view of one place, `fixture`, with `workers` people working.
    fn fixture_view(workers: u64) -> Arc<crate::game::view::EconomyView> {
        Arc::new(crate::game::view::EconomyView {
            places: vec![crate::game::view::PlaceView {
                id: "fixture".to_owned(),
                workers,
                ..crate::game::view::PlaceView::default()
            }],
        })
    }

    /// The UI script of the view tests. Each `game_time` call reads the view of its own revision, and of the revision the call before
    /// it carried (pending and pinned, or the last completed: served either way). An `Opened` event reads the newest revision the
    /// script has seen, and then runs `probe` (Lua that may call `window.record`).
    fn reads_views(probe: &str) -> String {
        format!(
            r"
        latest = 0
        local function refusal(revision)
          local ok, err = pcall(game.place, 'fixture', revision)
          if ok then return 'served' end
          return tostring(err):match('game%.place: [^\n]*')
        end
        game.on_time(function(t)
          local previous = latest
          latest = t.economy_revision
          window.hold()
          local own = tostring(game.place('fixture', t.economy_revision).workers)
          local before = 'none'
          if previous ~= 0 then before = tostring(game.place('fixture', previous).workers) end
          window.record('time', tostring(t.economy_revision), own, before)
        end)
        window.on_opened(function()
          window.record('opened', tostring(latest), tostring(game.place('fixture', latest).workers))
          {probe}
        end)
    "
        )
    }

    /// The held-UI witness (design §6): the UI VM is held inside the first of three `game_time` calls while the link has handed it
    /// three snapshots with three different views. Each call, when it runs, reads the view of its own revision however much newer
    /// the link's latest is, and the one before it; a selection-like event after them (here `Opened`) reads the newest revision the
    /// script has seen.
    #[test]
    fn three_pending_game_time_calls_each_read_their_own_view_and_a_later_event_reads_the_newest() {
        let gate = Gate::new();
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            Arc::new(move |_| {
                gate.wait();
                Ok(Vec::new())
            })
        };
        let mut timed = timed(
            &reads_views(""),
            Extras {
                functions: vec![("hold", hold)],
                ..Extras::default()
            },
        );
        for revision in 1..=3_u64 {
            let hour = u8::try_from(revision).unwrap();
            timed.slot.publish(snapshot_with_view(
                hour,
                revision,
                fixture_view(revision * 10),
            ));
            let _ = timed.link.take_answer();
        }
        assert_eq!(timed.link.outstanding(), 3, "three held calls");
        // The lifecycle place is free: the event waits behind the three calls, as a selection would.
        timed
            .link
            .post(UiEvent::Opened)
            .expect("the reserved place");
        gate.open();
        timed.pump_until(4);
        assert_eq!(
            timed.seen(),
            [
                "time 1 10 none",
                "time 2 20 10",
                "time 3 30 20",
                "opened 3 30"
            ]
        );
    }

    /// A revision is refused as retired as soon as a later `game_time` call completes, with the cache nowhere near full, and one the
    /// link never delivered is refused as missing.
    #[test]
    fn a_revision_is_retired_when_a_later_game_time_call_completes_and_not_before() {
        let gate = Gate::new();
        gate.open();
        let hold: HostFunction = {
            let gate = Arc::clone(&gate);
            Arc::new(move |_| {
                gate.wait();
                Ok(Vec::new())
            })
        };
        let probe = "window.record('refusals', refusal(1), refusal(2), refusal(99))";
        let mut timed = timed(
            &reads_views(probe),
            Extras {
                functions: vec![("hold", hold)],
                ..Extras::default()
            },
        );
        timed
            .slot
            .publish(snapshot_with_view(1, 1, fixture_view(10)));
        timed.pump_until(1);
        // Only the first has completed: it is the last completed, and served.
        timed.link.post(UiEvent::Opened).expect("posted");
        timed.pump_until(3);
        assert_eq!(
            &timed.seen()[1..],
            [
                "opened 1 10",
                "refusals served game.place: revision 2 is missing game.place: revision 99 is missing"
            ]
        );
        // The second completes: the first is refused at once, though the cache holds two views at most.
        timed
            .slot
            .publish(snapshot_with_view(2, 2, fixture_view(20)));
        timed.pump_until(4);
        timed.link.post(UiEvent::Opened).expect("posted");
        timed.pump_until(6);
        assert_eq!(
            &timed.seen()[4..],
            [
                "opened 2 20",
                "refusals game.place: revision 1 is retired served game.place: revision 99 is missing"
            ]
        );
    }

    /// The tables Lua gets are the real ones: the shipped Ireland start, as the simulation builds its view, read through `game.place`.
    #[test]
    fn game_place_serves_the_shipped_start_through_the_real_extension() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario loads");
        let (state, _) = crate::game::economy::starting_economy(&assets, &directory, &scenario)
            .expect("the start builds");
        let rules = crate::game::economy::load_rules(&assets)
            .expect("the rules load")
            .expect("the shipped rules exist");
        let ids: Vec<String> = scenario
            .locations
            .iter()
            .map(|location| location.id.clone())
            .collect();
        let view = Arc::new(crate::game::view::build(
            Some(&rules),
            &state,
            &ids,
            ck_sim::Date {
                year: 768,
                month: 5,
                day: 1,
                hour: 0,
            },
        ));
        let armagh = view.place("armagh").expect("armagh is in the view");
        let mut timed = timed(
            "game.on_time(function(t) local p = game.place('armagh', t.economy_revision) \
             window.record('armagh', tostring(p.workers + p.dependents), tostring(#p.pops), tostring(#p.buildings), \
             tostring(#p.prices), tostring(p.pops[1].kind), tostring(p.buildings[1].kind)) end)",
            Extras::default(),
        );
        timed
            .slot
            .publish(snapshot_with_view(1, 1, Arc::clone(&view)));
        timed.pump_until(1);
        assert_eq!(
            timed.seen(),
            [format!(
                "armagh {} {} {} {} {} {}",
                armagh.workers + armagh.dependents,
                armagh.pops.len(),
                armagh.buildings.len(),
                armagh.prices.len(),
                armagh.pops[0].kind,
                armagh.buildings[0].kind
            )]
        );
        assert!(armagh.workers > 0 && !armagh.pops.is_empty() && !armagh.buildings.is_empty());
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
                        crate::game::controls::SimControls::new(std::sync::mpsc::channel().0),
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
                views: Arc::default(),
                controls: crate::game::controls::SimControls::new(sender),
                slot: Arc::clone(&slot),
                time_registered,
                hover_registered,
                select_registered: Arc::new(AtomicBool::new(false)),
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

        let pointer = PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let mut tick = known_tick();
        tick.metrics.points = (400, 240);
        tick.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        tick.input.pointer_position = Some(pointer);
        tick.input.pointer_events = vec![
            PointerEvent::Scroll {
                position: pointer,
                amount: 15 * 256,
                unit: ScrollUnit::Points,
            },
            PointerEvent::Move(pointer),
        ];
        crate::map_app::lock(&map)
            .update(&tick)
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
                        crate::game::controls::SimControls::new(std::sync::mpsc::channel().0),
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
                views: Arc::default(),
                controls: crate::game::controls::SimControls::new(sender),
                slot: Arc::clone(&slot),
                time_registered,
                hover_registered,
                select_registered: Arc::new(AtomicBool::new(false)),
            });

        // The first frame is the ready answer at the head of the VM queue. A marker hover is
        // delivered after it, making time the next fair choice when that frame frees one place.
        link.post(update(1)).expect("first frame is posted");
        let pointer = PointerPosition {
            x: 200 * 256,
            y: 120 * 256,
        };
        let mut marker = known_tick();
        marker.metrics.points = (400, 240);
        marker.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        marker.input.pointer_position = Some(pointer);
        marker.input.pointer_events = vec![PointerEvent::Move(pointer)];
        crate::map_app::lock(&map)
            .update(&marker)
            .expect("the marker hover is produced");
        link.deliver_updates();
        assert_eq!(link.outstanding_names(), ["frame_update", "game_hover"]);

        link.post(update(3))
            .expect("second frame fills the last place");
        slot.publish(snapshot_at(1, 1, None));

        // Keep changing the latest hover while all places are occupied. The already-posted hover
        // remains in flight, so the latest hover cannot be sent until that result is drained.
        let mut latest = known_tick();
        latest.metrics.points = (400, 240);
        latest.metrics.pixels = PixelSize {
            width: 800,
            height: 480,
        };
        for position in [
            PointerPosition {
                x: 700 * 256,
                y: 400 * 256,
            },
            pointer,
            PointerPosition {
                x: 700 * 256,
                y: 400 * 256,
            },
        ] {
            latest.input.pointer_position = Some(position);
            latest.input.pointer_events = vec![PointerEvent::Move(position)];
            crate::map_app::lock(&map)
                .update(&latest)
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
