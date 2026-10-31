//! The GLFW and Vulkan path (`glfw-vulkan`): `ck-window-glfw`'s window and `ck-gpu-vulkan`'s presenter,
//! joined for `ck-window`'s loop (design §3, §4.1, §7).
//!
//! The loop wants one [`Backend`] and one [`Gpu2d`] (the `ck-render-2d` backend that [`Renderer`] turns into a
//! `ck_window::Presenter`) that share the window's handles. This path has more
//! to own than the native one: the Vulkan loader, the GLFW backend, the instance and the surface. They are
//! owned here, by one [`Run`] shared (on the main thread) by [`VulkanBackend`] and [`VulkanAdapter`], and
//! reached through the [`Stack`] seam, which has two implementations: [`real::Real`] (the accepted crates,
//! with its five `unsafe` operations, in the `vulkan` feature) and the fakes of the failure-boundary
//! tests (the tests of this file's `tests` module run the very same code with them).
//!
//! **The quad.** [`VulkanAdapter`] is the [`Gpu2d`] backend itself (milestone 4.5): the stack provides a
//! [`QuadSide`] for the run (the real one is `ck-render-2d`'s resource-aware `VulkanSinkSide::with_resources`, the fake one
//! records every scene it is given), `new_sink` gets the sink from it, `start` accepts only that sink and takes the
//! matching frame slot and the side's resident driver (both **before** anything starts: a failure of either is a start
//! failure) and hands them to [`Stack::start`], then connects the relay to the started presenter's texture wake, once;
//! the relay is disconnected on every end path (once for the run); and `take_report` returns the side's refusal before
//! anything the presenter has. The seam
//! names no `ck-gpu-vulkan` type (the generic code builds and is tested without the `vulkan` feature): the slot
//! is the stack's own associated type, and [`Running`] carries the drawn generation and the format notice as
//! plain values. The adapter's presenter wake and the side's refusal wake are both built from the run's one
//! notifier, in [`run_closure_with`].
//!
//! **The order of things.** `ck_window::run` creates the backend lazily, with the resource latch set first
//! ([`windowed::run_window`]); [`VulkanBackend::open`] then loads the Vulkan [`Stack::Loader`] **first**
//! (the first object of the run), gives GLFW a clone of it (through the `Stack`), and only then is the window
//! created. After the window exists, `create_window` creates the instance with the window's required
//! extensions and the surface; the adapter starts the presenter on them. Teardown follows the loop: when the
//! presenter's end is observed `Proven` ([`VulkanAdapter`]'s `poll_end`), the adapter releases the
//! presenter's handle and destroys the surface and the instance through the helper of `ck-gpu-vulkan`
//! (which refuses unless the presenter was proven), **before** the loop closes the window, so that a refusal
//! is reported as an unproven end and no `window closed` record is logged for a window that is kept. Then
//! [`VulkanBackend::destroy_window`] destroys the GLFW window, and [`VulkanBackend::terminate`] runs
//! `glfwTerminate` (which also resets GLFW's loader to NULL and releases its clone of the loader), and only
//! then is the run's own loader released, **last**. (A setup that failed before a presenter ran is
//! destroyed by `create_window` itself, or by `destroy_window` through the same helper.)
//!
//! **Nothing is released on an unproven end.** When the presenter's end is unproven the loop never calls
//! `destroy_window` or `terminate` and leaks the backend and the adapter, and with them this [`Run`]: the
//! loader, the instance, the surface and the window stay until the process's immediate exit. When the
//! helper **refuses** (`DestroyRefused`), or a partial setup cannot destroy what it created, [`Run::retain`]
//! forgets every owner this path holds (the instance the refusal returned, the surface unless the refusal
//! says it is already gone, the window, the GLFW backend, every loader clone), records the cause, and the
//! run ends with `RunEnd::Failed` and the incomplete-shutdown status (3), like an unproven presenter.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_log::{Admission, Level};
use ck_render_2d::{Gpu2d, QuadSink, Renderer, WakeRelay};
use ck_resources::ResourceService;
use ck_runtime::{RunContext, RunEnd};
use ck_window::{
    Backend, CleanupResult, CreatedWindow, NativeEvent, NativeWake, PixelSize, PresenterReport,
    Wait, WindowDesc,
};

use crate::config::ResourceConfig;
use crate::lifecycle::ResourceLatch;
use crate::windowed::{self, RunClosure, RunEnv, RunNotes, VulkanFigures};

#[cfg(test)]
mod fake;
#[cfg(test)]
mod logging;
#[cfg(test)]
mod quad_tests;
#[cfg(feature = "vulkan")]
mod real;
#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod tests;

/// What the presenter's proven end lets the destroy helper rely on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    /// No presenter was ever started (a partial setup, or the presenter's start failed).
    NoPresenter,
    /// The presenter reported a proven end.
    Proven,
}

/// A destroy helper that kept the instance (`DestroyRefused` in the real stack).
pub(crate) struct Refused<I> {
    /// The instance, untouched.
    pub(crate) instance: I,
    /// Whether the surface was destroyed before the refusal: it is then gone and is never destroyed again.
    pub(crate) surface_destroyed: bool,
    /// Why the instance was kept.
    pub(crate) cause: String,
}

/// What the run's resource path needs of the quad side and of the presenter's start, as plain values (no `ck-gpu-vulkan`
/// type): the service, the relay its wakes go through, and the limits parsed from `[resources]` and `[render]`.
#[derive(Clone)]
#[cfg_attr(
    not(feature = "vulkan"),
    expect(
        dead_code,
        reason = "only the real stack reads the run's resources; the fakes ignore them"
    )
)]
pub(crate) struct Resources {
    /// The run's resource service.
    pub(crate) service: ResourceService,
    /// The late-bound relay the service's upload and retirement wakes go through.
    pub(crate) relay: WakeRelay,
    /// The parsed `[resources]` limits.
    pub(crate) config: ResourceConfig,
    /// `[render] max_quads`.
    pub(crate) max_quads: usize,
}

/// The quad side of a run: everything of the backend except the presenter (the real one is `ck-render-2d`'s
/// `VulkanSinkSide::with_resources`). It holds no presenter, no handle and no GPU object.
pub(crate) trait QuadSide {
    /// What `Stack::start` takes of the sink this side made: the frame slot the presenter's render thread reads.
    type Slot;
    /// What `Stack::start` takes of the side's resource service: the driver the render thread's ledger works through.
    type Driver;
    /// A new resource-aware sink (and frame slot); it replaces any earlier one as "the one this side made".
    fn new_sink(&mut self) -> Arc<dyn QuadSink>;
    /// The frame slot behind `sink`, if it is the one `new_sink` made; an error otherwise.
    fn accept_frames(&self, sink: &Arc<dyn QuadSink>) -> Result<Self::Slot, String>;
    /// The driver over the side's resource service; an error only for a side that has none.
    fn resident_driver(&self) -> Result<Self::Driver, String>;
    /// Connects the relay to the started presenter's texture wake. An error only for a side with no service.
    fn connect(&self, texture_wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), String>;
    /// Disconnects the relay; harmless when repeated. An error only for a side with no service.
    fn disconnect(&self) -> Result<(), String>;
    /// The recorded refusal, once, as a `Failed` report.
    fn take_report(&self) -> Option<PresenterReport>;
}

/// The presenter handle the stack started: `ck_window::Presenter` without its `start`, plus what only a quad
/// drawing presenter has, as plain values.
pub(crate) trait Running {
    /// See [`ck_window::Presenter::set_clear`].
    fn set_clear(&mut self, rgb: [f32; 3]);
    /// See [`ck_window::Presenter::set_pixel_size`].
    fn set_pixel_size(&mut self, width: u32, height: u32);
    /// See [`ck_window::Presenter::request_redraw`].
    fn request_redraw(&mut self);
    /// See [`ck_window::Presenter::take_report`].
    fn take_report(&mut self) -> Option<PresenterReport>;
    /// See [`ck_window::Presenter::request_stop`].
    fn request_stop(&mut self);
    /// See [`ck_window::Presenter::poll_end`].
    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult>;
    /// The generation of the scene the last queued frame drew, if one was queued (read without waiting).
    fn drawn_generation(&self) -> Option<u64>;
    /// The format notice, once: the name of the surface format the quad is not drawn on.
    fn take_notice(&mut self) -> Option<String>;
    /// The wake the relay is connected to, so the service's uploads and retirements reach the render thread.
    fn texture_wake(&self) -> Arc<dyn Fn() + Send + Sync>;
    /// The presenter's last numbers, copied before its handle is released (`None`: it has none to give).
    fn figures(&self) -> Option<VulkanFigures>;
}

/// Everything this path needs from the accepted crates, behind one seam: the real stack, or a fake.
pub(crate) trait Stack {
    /// The native handles of the window.
    type Handles;
    /// The GLFW backend.
    type Glfw: Backend<Handles = Self::Handles>;
    /// The Vulkan loader (the real one is reference counted: this is the run's own clone).
    type Loader;
    /// The Vulkan instance.
    type Instance;
    /// The surface (a plain handle: copying it carries no ownership).
    type Surface: Copy;
    /// The started presenter.
    type Started: Running;
    /// What the presenter takes of the quad side's sink.
    type Slot;
    /// What the presenter takes of the quad side's resource service.
    type Driver;
    /// The quad side of a run.
    type Side: QuadSide<Slot = Self::Slot, Driver = Self::Driver>;

    /// Loads the Vulkan loader, by an explicit absolute path.
    fn load_loader(&self) -> Result<Self::Loader, String>;
    /// Creates the GLFW backend, giving GLFW the loader (before its `glfwInit`).
    fn glfw(&self, loader: &Self::Loader) -> Result<Self::Glfw, String>;
    /// Creates the instance with the window's required extensions.
    fn create_instance(
        &self,
        loader: &Self::Loader,
        handles: &Self::Handles,
    ) -> Result<Self::Instance, String>;
    /// Creates the surface on the main thread.
    fn create_surface(
        &self,
        handles: &Self::Handles,
        instance: &Self::Instance,
    ) -> Result<Self::Surface, String>;
    /// The quad side of a run, given the loop's wake (the notifier's), which a refusal calls, and the run's resources.
    fn quad_side(&self, wake: Arc<dyn Fn() + Send + Sync>, resources: &Resources) -> Self::Side;
    /// Starts the presenter on the surface, with the slot of the sink the quad side made, the side's driver and the run's
    /// resources (their limits). The presenter's device context is made here, after the surface exists.
    #[expect(
        clippy::too_many_arguments,
        reason = "the surface's start arguments plus the side's slot and driver and the run's resources"
    )]
    fn start(
        &self,
        instance: &Self::Instance,
        surface: Self::Surface,
        size: PixelSize,
        clear: [f32; 3],
        wake: Arc<dyn Fn() + Send + Sync>,
        slot: Self::Slot,
        driver: Self::Driver,
        resources: &Resources,
    ) -> Result<Self::Started, String>;
    /// Destroys the surface (when given) and then the instance, only after the presenter's proven end.
    fn destroy(
        &self,
        instance: Self::Instance,
        surface: Option<Self::Surface>,
        ended: Ended,
    ) -> Result<(), Refused<Self::Instance>>;
}

/// What the run owns besides the GLFW backend (which the backend wrapper holds).
struct State<S: Stack> {
    /// The run's own clone of the loader: released last.
    loader: Option<S::Loader>,
    instance: Option<S::Instance>,
    surface: Option<S::Surface>,
    /// The presenter's handle: released first at the teardown.
    presenter: Option<S::Started>,
    /// The presenter reported a proven end.
    proven: bool,
    /// The relay was disconnected (it is, once, on whichever end path the run takes first).
    disconnected: bool,
    /// Why the owners were retained (see the module documentation), if they were.
    retained: Option<String>,
}

/// The run's shared state and the stack it is built from.
struct Run<S: Stack> {
    stack: S,
    /// The quad side: the sink, its slot, the driver, the relay and the recorded refusal. It is the run's, not the adapter's,
    /// because the end paths that retain the owners (the backend's) disconnect the relay too.
    side: RefCell<S::Side>,
    /// The run's resources: what the presenter's start reads the limits from.
    resources: Resources,
    /// Where the presenter's last numbers go when its end is observed.
    notes: Rc<RunNotes>,
    state: RefCell<State<S>>,
}

impl<S: Stack> Run<S> {
    fn new(
        stack: S,
        wake: Arc<dyn Fn() + Send + Sync>,
        resources: Resources,
        notes: Rc<RunNotes>,
    ) -> Rc<Self> {
        let side = RefCell::new(stack.quad_side(wake, &resources));
        Rc::new(Self {
            stack,
            side,
            resources,
            notes,
            state: RefCell::new(State {
                loader: None,
                instance: None,
                surface: None,
                presenter: None,
                proven: false,
                disconnected: false,
                retained: None,
            }),
        })
    }

    /// Disconnects the relay, once for the run: the first end path to get here does it, a later one finds it done. An `Err`
    /// (a side with no service, which this path's side never is: it is a broken invariant) is logged once at `error` level as
    /// `render: <reason>` through the control path (bounded, and an error record has the logger's reserve), and the end path
    /// carries on.
    fn disconnect_relay(&self) {
        {
            let mut state = self.state.borrow_mut();
            if state.disconnected {
                return;
            }
            state.disconnected = true;
        }
        if let Err(reason) = self.side.borrow().disconnect() {
            crate::lifecycle::emit(Level::Error, "render", format_args!("{reason}"));
        }
    }

    /// Why the owners were retained, if they were.
    fn retained(&self) -> Option<String> {
        self.state.borrow().retained.clone()
    }

    /// Forgets every owner this path holds, the GLFW backend (if given) and the refusal's instance (if
    /// given) included, and records the cause. Nothing is dropped: no `Drop` runs for them, and no unwind can
    /// release them. A second retention adds its cause.
    fn retain(
        &self,
        glfw: Option<S::Glfw>,
        instance: Option<S::Instance>,
        surface: Option<S::Surface>,
        cause: String,
    ) {
        let mut state = self.state.borrow_mut();
        state.retained = Some(match state.retained.take() {
            Some(first) => format!("{first}; {cause}"),
            None => cause,
        });
        std::mem::forget((
            glfw,
            instance,
            surface,
            state.instance.take(),
            state.presenter.take(),
            state.loader.take(),
        ));
        state.surface = None;
    }
}

/// `ck-window`'s [`Backend`] over GLFW, the instance and the surface.
pub(crate) struct VulkanBackend<S: Stack> {
    /// The GLFW backend; `None` once it was retained.
    glfw: Option<S::Glfw>,
    run: Rc<Run<S>>,
}

impl<S: Stack> VulkanBackend<S> {
    /// Loads the loader first, then creates the GLFW backend with it: the first objects of the run (the
    /// resource latch is already set). A failure releases what was created, in reverse order.
    fn open(run: Rc<Run<S>>) -> Result<Self, String> {
        let loader = run
            .stack
            .load_loader()
            .map_err(|error| format!("cannot load the Vulkan loader: {error}"))?;
        let glfw = match run.stack.glfw(&loader) {
            Ok(glfw) => glfw,
            Err(error) => {
                drop(loader);
                return Err(format!("cannot start GLFW: {error}"));
            }
        };
        run.state.borrow_mut().loader = Some(loader);
        Ok(Self {
            glfw: Some(glfw),
            run,
        })
    }

    /// Retains every owner (see [`Run::retain`]).
    fn retain(
        &mut self,
        instance: Option<S::Instance>,
        surface: Option<S::Surface>,
        cause: String,
    ) {
        self.run.retain(self.glfw.take(), instance, surface, cause);
    }
}

impl<S: Stack> Backend for VulkanBackend<S> {
    type Handles = S::Handles;

    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<S::Handles>, String> {
        let Some(glfw) = self.glfw.as_mut() else {
            return Err("the Vulkan path was retained".to_owned());
        };
        let created = glfw.create_window(desc)?;
        let instance = {
            let state = self.run.state.borrow();
            let Some(loader) = state.loader.as_ref() else {
                return Err("the Vulkan loader is gone".to_owned());
            };
            self.run.stack.create_instance(loader, &created.handles)
        };
        let instance = match instance {
            Ok(instance) => instance,
            Err(error) => {
                // Partial setup: the window is destroyed (the loop's `terminate` follows).
                glfw.destroy_window();
                return Err(format!("cannot create the Vulkan instance: {error}"));
            }
        };
        match self.run.stack.create_surface(&created.handles, &instance) {
            Ok(surface) => {
                let mut state = self.run.state.borrow_mut();
                state.instance = Some(instance);
                state.surface = Some(surface);
                Ok(created)
            }
            Err(error) => {
                // Partial setup: the instance (no surface, no presenter), then the window, in this order.
                match self.run.stack.destroy(instance, None, Ended::NoPresenter) {
                    Ok(()) => {
                        if let Some(glfw) = self.glfw.as_mut() {
                            glfw.destroy_window();
                        }
                        Err(format!("cannot create the surface: {error}"))
                    }
                    Err(refused) => {
                        let cause = format!(
                            "the partial setup could not be destroyed: {}",
                            refused.cause
                        );
                        self.retain(Some(refused.instance), None, cause);
                        Err(format!("cannot create the surface: {error}"))
                    }
                }
            }
        }
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        match &self.glfw {
            Some(glfw) => glfw.wake_target(),
            None => Arc::new(NoWake),
        }
    }

    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, cap: usize) {
        if let Some(glfw) = self.glfw.as_mut() {
            glfw.pump(events, wait, cap);
        }
    }

    /// GLFW waits until a deadline (CK-155's `pump_until`), which the loop's paced update source needs: this wrapper says so
    /// and forwards, or a run with a `[frame]` update source would reach the trait's default and stop.
    fn supports_deadline(&self) -> bool {
        self.glfw.as_ref().is_some_and(Backend::supports_deadline)
    }

    fn pump_until(&mut self, events: &mut dyn FnMut(NativeEvent), deadline: Instant, cap: usize) {
        if let Some(glfw) = self.glfw.as_mut() {
            glfw.pump_until(events, deadline, cap);
        }
    }

    fn destroy_window(&mut self) {
        let (instance, surface, ended, presenter) = {
            let mut state = self.run.state.borrow_mut();
            if state.retained.is_some() {
                return;
            }
            (
                state.instance.take(),
                state.surface.take(),
                if state.proven {
                    Ended::Proven
                } else {
                    Ended::NoPresenter
                },
                state.presenter.take(),
            )
        };
        if presenter.is_some() && ended != Ended::Proven {
            // A started presenter whose end was not proven: nothing is destroyed (not reached through the
            // loop, which only tears down after a proven end).
            self.run.disconnect_relay();
            self.retain(
                instance,
                surface,
                "the presenter's end was not proven".to_owned(),
            );
            std::mem::forget(presenter);
            return;
        }
        // The presenter's handle is released first (its loader clone with it).
        drop(presenter);
        let Some(instance) = instance else {
            if let Some(glfw) = self.glfw.as_mut() {
                glfw.destroy_window();
            }
            return;
        };
        match self.run.stack.destroy(instance, surface, ended) {
            Ok(()) => {
                if let Some(glfw) = self.glfw.as_mut() {
                    glfw.destroy_window();
                }
            }
            Err(refused) => {
                // The surface is kept unless the refusal says it is already gone: never destroyed twice.
                let surface = if refused.surface_destroyed {
                    None
                } else {
                    surface
                };
                let cause = format!(
                    "the instance could not be destroyed ({}; the surface was {})",
                    refused.cause,
                    if refused.surface_destroyed {
                        "already destroyed"
                    } else {
                        "kept"
                    }
                );
                self.retain(Some(refused.instance), surface, cause);
            }
        }
    }

    fn terminate(&mut self) {
        if self.run.state.borrow().retained.is_some() {
            return;
        }
        if let Some(glfw) = self.glfw.as_mut() {
            glfw.terminate();
        }
        // After `glfwTerminate` (and GLFW's NULL reset): the run's own clone of the loader, last.
        let loader = self.run.state.borrow_mut().loader.take();
        drop(loader);
    }
}

/// A wake target for a backend that was retained: it does nothing.
struct NoWake;

impl NativeWake for NoWake {
    fn wake(&self) {}
}

/// The [`Gpu2d`] backend over the stack's started presenter and quad side.
///
/// The surface and the instance are destroyed **here**, when the presenter's end is observed `Proven`
/// ([`Gpu2d::poll_end`]), and not in the backend's `destroy_window`: if the helper refuses, the end is
/// reported `Unproven`, so the loop destroys nothing, leaks everything and logs no `window closed`, rather
/// than closing a window that is kept. The reports the presenter published are taken at that moment (the
/// thread has ended, so the sequence is finite) and served from a queue once its handle is released.
///
/// **The format notice** (design §7.3). Every [`Gpu2d`] call first takes the notice from the presenter
/// ([`Running::take_notice`]) and logs it as one
/// `warn render: textured quad skipped: the glfw-vulkan surface format is <format>, not UNORM` record through
/// `try_log`, which never waits. While the run is active a refused record is kept and tried again at the next
/// call; on the end paths (`request_stop`, `poll_end`) a refused record is given up (the logger counts the
/// drop). When `poll_end` observes the end, the notice is taken once more **before** the remaining reports are
/// drained and the presenter's handle is released (or retained, on an unproven end), so a notice recorded after
/// the last ordinary call is not lost; any time spent logging comes out of `poll_end`'s own budget.
///
/// **The first-frame record.** When the loop first takes `FirstFrameQueued`, and the presenter has a drawn
/// generation, one `debug render: first frame drawn, generation N` record is logged, the same as the native
/// path's. It is the latest generation drawn, not a history, and claims nothing about the screen.
pub(crate) struct VulkanAdapter<S: Stack> {
    run: Rc<Run<S>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The reports taken when the end was observed, not yet handed to the loop.
    queued: VecDeque<PresenterReport>,
    /// The final result of [`Gpu2d::poll_end`], once observed.
    ended: Option<CleanupResult>,
    /// The format notice taken from the presenter and not logged yet.
    notice: Option<String>,
    /// The first frame was reported and its record has not been written yet.
    first_frame_pending: bool,
    /// The presenter's drawn generation as of the end (its handle is gone after that).
    drawn_at_end: Option<u64>,
}

impl<S: Stack> VulkanAdapter<S> {
    /// An adapter whose presenter calls `wake` (the notifier's, built once per run; the run's quad side was built with the same).
    fn new(run: Rc<Run<S>>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            run,
            wake,
            queued: VecDeque::new(),
            ended: None,
            notice: None,
            first_frame_pending: false,
            drawn_at_end: None,
        }
    }

    /// The presenter ended `Proven` (or was never started): releases its handle, then destroys the surface
    /// and the instance through the helper. A refusal retains everything and says why.
    fn destroy_after_the_presenter(&mut self) -> CleanupResult {
        let (presenter, instance, surface) = {
            let mut state = self.run.state.borrow_mut();
            state.proven = true;
            (
                state.presenter.take(),
                state.instance.take(),
                state.surface.take(),
            )
        };
        let ended = if presenter.is_some() {
            Ended::Proven
        } else {
            Ended::NoPresenter
        };
        drop(presenter);
        if ended == Ended::Proven {
            // After the presenter's handle is released and before the surface and the instance are destroyed.
            self.run.disconnect_relay();
        }
        let Some(instance) = instance else {
            return CleanupResult::Proven;
        };
        match self.run.stack.destroy(instance, surface, ended) {
            Ok(()) => CleanupResult::Proven,
            Err(refused) => {
                // The surface is kept unless the refusal says it is already gone: never destroyed twice.
                let surface = if refused.surface_destroyed {
                    None
                } else {
                    surface
                };
                let cause = format!(
                    "the instance could not be destroyed ({}; the surface was {})",
                    refused.cause,
                    if refused.surface_destroyed {
                        "already destroyed"
                    } else {
                        "kept"
                    }
                );
                self.run
                    .retain(None, Some(refused.instance), surface, cause.clone());
                CleanupResult::Unproven { reason: cause }
            }
        }
    }

    /// Takes the side's slot and driver, then has the stack start the presenter on the instance and the surface.
    fn start_presenter(
        &self,
        sink: &Arc<dyn QuadSink>,
        desc: &WindowDesc,
        pixel_size: PixelSize,
    ) -> Result<S::Started, String> {
        let (slot, driver) = {
            let side = self.run.side.borrow();
            (side.accept_frames(sink)?, side.resident_driver()?)
        };
        let state = self.run.state.borrow();
        let (Some(instance), Some(surface)) = (state.instance.as_ref(), state.surface) else {
            return Err("the instance and the surface do not exist".to_owned());
        };
        self.run.stack.start(
            instance,
            surface,
            pixel_size,
            desc.clear,
            Arc::clone(&self.wake),
            slot,
            driver,
            &self.run.resources,
        )
    }

    fn with<T>(&self, call: impl FnOnce(&mut S::Started) -> T) -> Option<T> {
        self.run.state.borrow_mut().presenter.as_mut().map(call)
    }

    /// Takes the presenter's notice (if it has one) and logs it, if there is one to log; says whether it tried.
    /// Called first thing by every [`Gpu2d`] method, all of which run on the main thread.
    ///
    /// **It never waits:** the record goes through `try_log`, which takes or drops it at once. Off the end paths
    /// a dropped record is kept and tried again at the next call; on the end paths (`closing`) it is given up, as
    /// the run is over, and the logger has counted the drop. A [`Gpu2d`] method's own deadline is therefore never
    /// spent on the logger.
    fn announce(&mut self, closing: bool) -> bool {
        if self.notice.is_none() {
            self.notice = self.with(Running::take_notice).flatten();
        }
        let Some(format) = &self.notice else {
            return false;
        };
        let admission = ck_log::try_log!(
            Level::Warn,
            "render",
            "textured quad skipped: the glfw-vulkan surface format is {format}, not UNORM"
        );
        if admission != Admission::Dropped || closing {
            self.notice = None;
        }
        true
    }

    /// The first-frame record, once the first frame was reported and the presenter has a drawn generation.
    fn first_frame_record(&mut self) {
        if !self.first_frame_pending {
            return;
        }
        let generation = self
            .with(|presenter| presenter.drawn_generation())
            .flatten()
            .or(self.drawn_at_end);
        if let Some(generation) = generation {
            self.first_frame_pending = false;
            let _ = ck_log::try_log!(
                Level::Debug,
                "render",
                "first frame drawn, generation {generation}"
            );
        }
    }
}

impl<S: Stack> Gpu2d for VulkanAdapter<S> {
    type Handles = S::Handles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.announce(false);
        self.run.side.borrow_mut().new_sink()
    }

    fn start(
        &mut self,
        _handles: S::Handles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.announce(false);
        // Nothing starts before the side has given both its slot and its driver: only the sink this side made is accepted, and
        // a side with no service has no driver. Any failure here, or in the start, is a start failure with the relay
        // disconnected (nothing was connected) before the error returns.
        let started = self.start_presenter(&sink, desc, pixel_size);
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                self.run.disconnect_relay();
                return Err(error);
            }
        };
        let texture_wake = started.texture_wake();
        self.run.state.borrow_mut().presenter = Some(started);
        // After a successful start, once: the service's uploads and retirements now reach the presenter's render thread.
        if let Err(reason) = self.run.side.borrow().connect(texture_wake) {
            crate::lifecycle::emit(Level::Error, "render", format_args!("{reason}"));
        }
        Ok(())
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.announce(false);
        self.with(|presenter| presenter.set_clear(rgb));
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.announce(false);
        self.with(|presenter| presenter.set_pixel_size(width, height));
    }

    fn request_redraw(&mut self) {
        self.announce(false);
        self.with(Running::request_redraw);
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.announce(false);
        // The side's refusal comes first, once, before anything the presenter has.
        if let Some(report) = self.run.side.borrow().take_report() {
            return Some(report);
        }
        let report = match self.queued.pop_front() {
            Some(report) => Some(report),
            None => self.with(Running::take_report).flatten(),
        };
        if matches!(report, Some(PresenterReport::FirstFrameQueued)) {
            self.first_frame_pending = true;
        }
        self.first_frame_record();
        report
    }

    fn request_stop(&mut self) {
        self.announce(true);
        self.with(Running::request_stop);
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        if let Some(done) = &self.ended {
            return Some(done.clone());
        }
        let started = Instant::now();
        // A notice still pending costs a moment: that moment comes out of the caller's budget.
        let within = if self.announce(true) {
            within.saturating_sub(started.elapsed())
        } else {
            within
        };
        // A presenter that was never started has nothing to prove.
        let observed = match self.with(|presenter| presenter.poll_end(within)) {
            Some(observed) => observed?,
            None => CleanupResult::Proven,
        };
        // The end was observed, so what the thread recorded is final. The notice first (a notice recorded
        // after the last ordinary call is not lost to the handle's removal), then the presenter's last numbers for the
        // final resource record (copied while the handle is alive, as `native.rs` does), then the reports, then the handle.
        self.announce(true);
        if let Some(figures) = self.with(|presenter| presenter.figures()).flatten() {
            *self.run.notes.vulkan_gpu.borrow_mut() = Some(figures);
        }
        self.drawn_at_end = self
            .with(|presenter| presenter.drawn_generation())
            .flatten();
        while let Some(report) = self.with(Running::take_report).flatten() {
            self.queued.push_back(report);
        }
        let result = match observed {
            CleanupResult::Proven => self.destroy_after_the_presenter(),
            unproven @ CleanupResult::Unproven { .. } => {
                // The handle stays (the loop retains everything); the relay goes first, so a late service wake reaches nobody.
                self.run.disconnect_relay();
                unproven
            }
        };
        self.ended = Some(result.clone());
        Some(result)
    }
}

/// The run closure of a windowed `glfw-vulkan` run over `stack`.
pub(crate) fn run_closure_with<S: Stack + 'static>(
    env: RunEnv,
    stack: S,
    latch: &'static ResourceLatch,
    drain: Duration,
) -> RunClosure {
    Box::new(move |_: &RunContext| -> RunEnd {
        let Some(service) = env.resources.clone() else {
            return windowed::fail_before_window(
                &env,
                "cannot start the presenter: the glfw-vulkan path has no resource service"
                    .to_owned(),
            );
        };
        let resources = Resources {
            service,
            relay: env.wake_relay.clone(),
            config: env.resource_config,
            max_quads: env.max_quads,
        };
        // The one wake of the run: the notifier's. The presenter's reports and the quad side's refusal both call
        // it (the run builds the side with the same one), so a sleeping loop is woken by either.
        let notifier = Arc::clone(&env.notifier);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || notifier.wake());
        let run = Run::new(stack, Arc::clone(&wake), resources, Rc::clone(&env.notes));
        let adapter = VulkanAdapter::new(Rc::clone(&run), wake);
        let presenter = Renderer::new(adapter, Arc::clone(&env.bridge));
        let opener = Rc::clone(&run);
        let kept = Rc::clone(&run);
        let end = windowed::run_window_keeping(
            &env,
            move || VulkanBackend::open(opener),
            presenter,
            latch,
            drain,
            &|| kept.retained().is_some(),
        );
        match run.retained() {
            None => end,
            Some(cause) => {
                // The owners are retained: the run is incomplete (status 3). `run_window_keeping` kept the
                // latch set from the loop's return on, the late-callback drain included.
                RunEnd::Failed(match end {
                    RunEnd::Failed(message) if message.contains(&cause) => message,
                    RunEnd::Failed(message) => format!("{message}; {cause}"),
                    RunEnd::Finished | RunEnd::Quit => {
                        format!("the Vulkan teardown is unproven: {cause}")
                    }
                })
            }
        }
    })
}

/// The run closure of a windowed `glfw-vulkan` run: the real stack.
#[cfg(feature = "vulkan")]
pub(crate) fn run_closure(env: RunEnv) -> RunClosure {
    run_closure_with(
        env,
        real::Real,
        &crate::lifecycle::RESOURCES,
        windowed::DRAIN,
    )
}
