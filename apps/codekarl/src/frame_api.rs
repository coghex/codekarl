//! The `frame` extension of the UI VM (design §8.2, §9): `frame.on_update(fn | nil)`.
//!
//! `frame.on_update(fn)` registers `fn` as the callback `frame_update` (the table `{dt, elapsed, sequence, ...}` of
//! `ui_link`) and turns the run's [`UpdateControl`] on; `frame.on_update(nil)` clears it and turns the control off. The
//! control is set by an observer `ck-lua` calls on the owner thread **after** the callback table changed
//! ([`Extension::callback_observed`]), through [`UpdateControl::set`], which advances the registration epoch and wakes the
//! loop, so a delayed "not registered" answer to an earlier registration cannot disable this one.
//!
//! Two backends:
//! - **a windowed run** (**native-metal** and **`glfw-vulkan`**): the observed setter, over the control of the loop's
//!   update source (`[frame]` pacing);
//! - **headless**: the callback is registered (and its registration published), and no update is ever posted (there is no window
//!   loop).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ck_lua::Extension;
use ck_window::UpdateControl;

use crate::window_api::FRAME_UPDATE;

/// Whether the UI VM has a `frame_update` callback registered, and which registration it is: one packed atomic, `generation << 1 |
/// registered`. The observer of `frame.on_update` is the only writer (one atomic read-modify-write that changes the generation and
/// the bit together), and the input tick reads the state with one `load`, so it never sees a new bit with an old generation or the
/// reverse. Generations are compared for inequality only; a 63-bit counter cannot wrap in practice.
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameRegistration(Arc<AtomicU64>);

impl FrameRegistration {
    /// Records a registration (`true`) or an unregistration (`false`): the next generation, with the new bit, in one step.
    pub(crate) fn publish(&self, registered: bool) {
        let _previous = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                Some((((old >> 1).wrapping_add(1)) << 1) | u64::from(registered))
            });
    }

    /// The state as `(generation, registered)`, read in one load.
    pub(crate) fn state(&self) -> (u64, bool) {
        let packed = self.0.load(Ordering::SeqCst);
        (packed >> 1, packed & 1 == 1)
    }
}

/// How the run gives the `frame` table its setter.
pub(crate) enum FrameBackend {
    /// A windowed run: the setter drives this control.
    Control(UpdateControl),
    /// headless: the callback is registered and nothing ever posts it.
    Headless,
}

/// The `frame` extension for `backend`.
pub(crate) fn extension(
    map_enabled: bool,
    backend: FrameBackend,
    registration: FrameRegistration,
) -> Extension {
    let frame = Extension::new("frame");
    match backend {
        FrameBackend::Control(control) => {
            if map_enabled {
                control.set(true);
            }
            frame.callback_observed(
                "on_update",
                FRAME_UPDATE,
                Arc::new(move |registered| {
                    // The registration first, so that when the loop wakes the input tick finds it published.
                    registration.publish(registered);
                    control.set(registered || map_enabled);
                }),
            )
        }
        // No window loop and no control, but the registration is published all the same, so a driver that plays the loop's part (the
        // tests' `Drive`) sees what a windowed run would.
        FrameBackend::Headless => frame.callback_observed(
            "on_update",
            FRAME_UPDATE,
            Arc::new(move |registered| registration.publish(registered)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packed_state_decodes_through_register_unregister_and_repeated_registers() {
        let registration = FrameRegistration::default();
        assert_eq!(registration.state(), (0, false));
        registration.publish(true);
        assert_eq!(registration.state(), (1, true));
        registration.publish(false);
        assert_eq!(registration.state(), (2, false));
        // Register and unregister between two reads: one generation change (two steps), the final bit.
        registration.publish(true);
        registration.publish(false);
        assert_eq!(registration.state(), (4, false));
        // Repeated registers: every publish changes the generation.
        let mut seen = vec![registration.state().0];
        for _ in 0..4 {
            registration.publish(true);
            seen.push(registration.state().0);
            assert!(registration.state().1);
        }
        assert!(seen.windows(2).all(|pair| pair[0] != pair[1]), "{seen:?}");
    }

    #[test]
    fn a_clone_shares_the_state() {
        let registration = FrameRegistration::default();
        let reader = registration.clone();
        registration.publish(true);
        assert_eq!(reader.state(), (1, true));
    }

    #[test]
    fn the_generation_wraps_inside_its_63_bits_without_touching_the_bit() {
        let registration = FrameRegistration(Arc::new(AtomicU64::new(u64::MAX)));
        registration.publish(false);
        // The top generation wrapped to zero then one; the bit is the one written.
        assert_eq!(registration.state(), (0, false));
        registration.publish(true);
        assert_eq!(registration.state(), (1, true));
    }
}
