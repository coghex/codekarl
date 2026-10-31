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
//! - **headless**: the plain setter: the callback is registered, and no update is ever posted (there is no window loop).

use std::sync::Arc;

use ck_lua::Extension;
use ck_window::UpdateControl;

use crate::window_api::FRAME_UPDATE;

/// How the run gives the `frame` table its setter.
pub(crate) enum FrameBackend {
    /// A windowed run: the setter drives this control.
    Control(UpdateControl),
    /// headless: the callback is registered and nothing ever posts it.
    Headless,
}

/// The `frame` extension for `backend`.
pub(crate) fn extension(backend: FrameBackend) -> Extension {
    let frame = Extension::new("frame");
    match backend {
        FrameBackend::Control(control) => frame.callback_observed(
            "on_update",
            FRAME_UPDATE,
            Arc::new(move |registered| control.set(registered)),
        ),
        FrameBackend::Headless => frame.callback("on_update", FRAME_UPDATE),
    }
}
