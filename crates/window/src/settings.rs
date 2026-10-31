//! The loop's settings: per-pass caps and teardown deadlines.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use ck_input::Bindings;

use crate::metrics::LoopMetrics;
use crate::notifier::Notifier;

/// Which setting was zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsError {
    /// The setting's name.
    pub setting: &'static str,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "the setting `{}` must not be zero", self.setting)
    }
}

impl Error for SettingsError {}

#[derive(Debug)]
struct UpdateControlState {
    enabled: bool,
    epoch: u64,
    notifier: Weak<Notifier>,
}

/// A thread-safe switch for a configured UI update source. Every change advances its registration epoch and wakes
/// the window loop after releasing the control lock.
#[derive(Clone)]
pub struct UpdateControl(Arc<Mutex<UpdateControlState>>);

impl fmt::Debug for UpdateControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("UpdateControl")
            .field("enabled", &state.enabled)
            .field("epoch", &state.epoch)
            .finish_non_exhaustive()
    }
}

impl Default for UpdateControl {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateControl {
    /// Creates a disabled control. The loop binds it to its lifetime-aware notifier when run starts.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(UpdateControlState {
            enabled: false,
            epoch: 0,
            notifier: Weak::new(),
        })))
    }

    fn lock(&self) -> MutexGuard<'_, UpdateControlState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets whether updates are enabled, always creating a new registration epoch and waking the loop.
    pub fn set(&self, enabled: bool) {
        let notifier = {
            let mut state = self.lock();
            state.enabled = enabled;
            state.epoch = state.epoch.saturating_add(1);
            state.notifier.upgrade()
        };
        if let Some(notifier) = notifier {
            notifier.wake();
        }
    }

    /// The current enabled flag and its registration identity.
    pub(crate) fn state(&self) -> (bool, u64) {
        let state = self.lock();
        (state.enabled, state.epoch)
    }

    pub(crate) fn bind(&self, notifier: &Arc<Notifier>) {
        self.lock().notifier = Arc::downgrade(notifier);
    }
}

/// The validated pacing configuration for UI updates.
#[derive(Debug, Clone)]
pub struct UpdateSettings {
    updates_per_second: u32,
    max_delta: Duration,
    period: Duration,
    control: UpdateControl,
    metrics: Option<LoopMetrics>,
}

impl PartialEq for UpdateSettings {
    fn eq(&self, other: &Self) -> bool {
        self.updates_per_second == other.updates_per_second
            && self.max_delta == other.max_delta
            && self.period == other.period
    }
}

impl Eq for UpdateSettings {}

impl UpdateSettings {
    /// Creates a pacing configuration. Rates must be 1 through 1,000 Hz and `max_delta` must be positive.
    ///
    /// # Errors
    ///
    /// Returns a [`SettingsError`] naming an out-of-range rate or a zero `max_delta`.
    pub fn new(updates_per_second: u32, max_delta: Duration) -> Result<Self, SettingsError> {
        if !(1..=1_000).contains(&updates_per_second) {
            return Err(SettingsError {
                setting: "updates_per_second",
            });
        }
        if max_delta.is_zero() {
            return Err(SettingsError {
                setting: "max_delta",
            });
        }
        let nanos = 1_000_000_000_u64
            .checked_div(u64::from(updates_per_second))
            .filter(|nanos| *nanos != 0)
            .ok_or(SettingsError {
                setting: "updates_per_second",
            })?;
        Ok(Self {
            updates_per_second,
            max_delta,
            period: Duration::from_nanos(nanos),
            control: UpdateControl::new(),
            metrics: None,
        })
    }

    /// These settings with the loop's measurement windows: the loop adopts requested windows at the start of a pass and counts the
    /// updates it delivers and the opportunities it coalesces into them. Without it nothing is counted.
    #[must_use]
    pub fn with_metrics(mut self, metrics: LoopMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// The loop's measurement windows, when given.
    #[must_use]
    pub const fn metrics(&self) -> Option<&LoopMetrics> {
        self.metrics.as_ref()
    }

    /// The configured target rate.
    #[must_use]
    pub const fn updates_per_second(&self) -> u32 {
        self.updates_per_second
    }

    /// The largest delivered frame delta.
    #[must_use]
    pub const fn max_delta(&self) -> Duration {
        self.max_delta
    }

    /// The representable whole-nanosecond target period.
    #[must_use]
    pub const fn period(&self) -> Duration {
        self.period
    }

    /// A cloneable handle for enabling and disabling the update callback.
    #[must_use]
    pub fn control(&self) -> UpdateControl {
        self.control.clone()
    }
}

/// The caps and deadlines of [`run`](crate::run), and the input bindings the loop's mapper uses. Nothing is
/// normalised: a zero is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopSettings {
    native_events: usize,
    items: usize,
    presenter_teardown: Duration,
    disarm: Duration,
    bindings: Bindings,
    updates: Option<UpdateSettings>,
}

impl LoopSettings {
    /// Settings with `native_events` (events a capped backend processes per pump), `items` (presenter
    /// reports, and UI answers, taken per pass), the presenter teardown deadline and the disarm deadline.
    ///
    /// # Errors
    ///
    /// [`SettingsError`] naming the first setting that is zero.
    pub fn new(
        native_events: usize,
        items: usize,
        presenter_teardown: Duration,
        disarm: Duration,
    ) -> Result<Self, SettingsError> {
        if native_events == 0 {
            return Err(SettingsError {
                setting: "native_events",
            });
        }
        if items == 0 {
            return Err(SettingsError { setting: "items" });
        }
        if presenter_teardown.is_zero() {
            return Err(SettingsError {
                setting: "presenter_teardown",
            });
        }
        if disarm.is_zero() {
            return Err(SettingsError { setting: "disarm" });
        }
        Ok(Self {
            native_events,
            items,
            presenter_teardown,
            disarm,
            bindings: default_bindings(),
            updates: None,
        })
    }

    /// These settings with `bindings` for the loop's input mapper (`new` and `default` use the design's defaults, so
    /// existing callers are unchanged).
    #[must_use]
    pub fn with_bindings(mut self, bindings: Bindings) -> Self {
        self.bindings = bindings;
        self
    }

    /// The input bindings the loop's mapper uses.
    #[must_use]
    pub const fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// These settings with a paced UI update source.
    #[must_use]
    pub fn with_updates(mut self, updates: UpdateSettings) -> Self {
        self.updates = Some(updates);
        self
    }

    /// The optional paced UI update configuration.
    #[must_use]
    pub const fn updates(&self) -> Option<&UpdateSettings> {
        self.updates.as_ref()
    }

    /// The most native events a capped backend processes per pump.
    #[must_use]
    pub const fn native_events(&self) -> usize {
        self.native_events
    }

    /// The most presenter reports, and the most UI answers, taken per pass.
    #[must_use]
    pub const fn items(&self) -> usize {
        self.items
    }

    /// How long teardown waits for the presenter to be observed finished.
    #[must_use]
    pub const fn presenter_teardown(&self) -> Duration {
        self.presenter_teardown
    }

    /// How long teardown waits for native wakes in flight when disarming.
    #[must_use]
    pub const fn disarm(&self) -> Duration {
        self.disarm
    }
}

/// The design's default bindings (`docs/scene-design.md` §7.1): validated, and so infallible for no entries.
fn default_bindings() -> Bindings {
    match Bindings::from_entries(std::iter::empty()) {
        Ok(bindings) => bindings,
        // `from_entries` with no entries only fills the defaults, which are valid by construction.
        Err(error) => unreachable!("the default bindings are valid: {error}"),
    }
}

impl Default for LoopSettings {
    /// 64 native events, 16 items, a 2 s presenter teardown, a 100 ms disarm and the default bindings.
    fn default() -> Self {
        Self {
            native_events: 64,
            items: 16,
            presenter_teardown: Duration::from_secs(2),
            disarm: Duration::from_millis(100),
            bindings: default_bindings(),
            updates: None,
        }
    }
}
