//! The `frame` extension of the UI VM (design §8.2, §9): `frame.on_update(fn | nil)`.
//!
//! `frame.on_update(fn)` registers `fn` as the callback `frame_update` (the table `{dt, elapsed, sequence, ...}` of
//! `ui_link`) and turns the run's [`UpdateControl`] on; `frame.on_update(nil)` clears it and turns the control off. The
//! control is set by an observer `ck-lua` calls on the owner thread **after** the callback table changed
//! ([`Extension::callback_observed`]), through [`UpdateControl::set`], which advances the registration epoch and wakes the
//! loop, so a delayed "not registered" answer to an earlier registration cannot disable this one.
//!
//! Three backends:
//! - **native-metal**: the observed setter, over the control of the loop's update source (`[frame]` pacing);
//! - **headless**: the plain setter: the callback is registered, and no update is ever posted (there is no window loop);
//! - **`glfw-vulkan`**: the unavailable setter ([`Extension::unavailable_callback`]): `frame.on_update(fn)` and
//!   `frame.on_update(nil)` raise "`frame.on_update` is not available on the glfw-vulkan backend until milestone 5.5" after
//!   exactly the setter's argument validation, before any change: no callback appears, the control and its epoch stay as
//!   they were.

use std::sync::Arc;

use ck_lua::Extension;
use ck_window::UpdateControl;

use crate::texture_api::unavailable;
use crate::window_api::FRAME_UPDATE;

/// How the run gives the `frame` table its setter.
pub(crate) enum FrameBackend {
    /// native-metal: the setter drives this control.
    Control(UpdateControl),
    /// headless: the callback is registered and nothing ever posts it.
    Headless,
    /// `glfw-vulkan`: the setter is not available.
    Unavailable,
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
        FrameBackend::Unavailable => {
            frame.unavailable_callback("on_update", &unavailable("frame.on_update"))
        }
    }
}
