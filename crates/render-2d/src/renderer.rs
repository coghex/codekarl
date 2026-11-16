//! The backend trait and the renderer: a `ck_window::Presenter` over one GPU backend.

use std::sync::Arc;
use std::time::Duration;

use ck_window::{CleanupResult, PixelSize, Presenter, PresenterReport, WindowDesc};

use crate::bridge::{QuadSink, SceneBridge};

/// What a backend adapter offers `ck-render-2d` (issue 4 implements it for each GPU crate).
///
/// It is everything [`Presenter`] offers, plus [`new_sink`](Self::new_sink), and its `start` receives that
/// sink. The sink is created **before** the render thread exists and already holds the scene when the thread
/// starts, so the thread reads its initial value before its first frame: a quad written before the window
/// opened is in the first frame. See [`Presenter`] for the contract of every other method; the renderer
/// forwards them unchanged.
pub trait Gpu2d {
    /// The native handles `ck_window`'s `create_window` hands over.
    type Handles;

    /// Creates the backend's latest-value sink. Called once, before [`start`](Self::start), on the thread that
    /// calls `start`.
    fn new_sink(&mut self) -> Arc<dyn QuadSink>;

    /// Starts the render thread, which reads `sink`, and returns at once.
    ///
    /// # Errors
    ///
    /// As [`Presenter::start`]: `Err` means no thread was spawned and no GPU object exists.
    fn start(
        &mut self,
        handles: Self::Handles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String>;

    /// As [`Presenter::set_clear`].
    fn set_clear(&mut self, rgb: [f32; 3]);

    /// Sets the drawable's size in pixels. Never waits.
    fn set_pixel_size(&mut self, width: u32, height: u32);

    /// As [`Presenter::set_size`]: the content size in points and the drawable's size in pixels together. A backend that needs
    /// only the pixels keeps this default, which passes them to [`set_pixel_size`](Self::set_pixel_size).
    fn set_size(&mut self, points: (u32, u32), pixels: PixelSize) {
        let _ = points;
        self.set_pixel_size(pixels.width, pixels.height);
    }

    /// As [`Presenter::request_redraw`].
    fn request_redraw(&mut self);

    /// Requests continuous drawing. Backends that do not support it keep their default no-op.
    fn set_draw_every_refresh(&mut self, enabled: bool) {
        let _ = enabled;
    }

    /// As [`Presenter::take_report`].
    fn take_report(&mut self) -> Option<PresenterReport>;

    /// Returns the backend's live shown-frame time reader when it has one.
    #[cfg(all(feature = "metal", target_os = "macos"))]
    fn presented_times_reader(&self) -> Option<crate::PresentedTimesReader> {
        None
    }

    /// As [`Presenter::request_stop`].
    fn request_stop(&mut self);

    /// As [`Presenter::poll_end`].
    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult>;
}

/// A [`Presenter`] over one [`Gpu2d`] backend, driving the scene bridge.
///
/// - **`start`** creates the backend's sink, attaches it to the bridge (which publishes the current scene), then
///   starts the backend with the sink. If the backend fails to start, the bridge is detached.
/// - **`set_size`** replaces the bridge's metrics, points and pixels together (which republishes), then forwards the pixels.
/// - **`request_stop`** detaches the bridge, then forwards. Detaching again is harmless.
/// - Everything else forwards unchanged, and the backend's report, end and cleanup semantics are untouched.
pub struct Renderer<G> {
    gpu: G,
    bridge: Arc<SceneBridge>,
}

impl<G: Gpu2d> Renderer<G> {
    /// A renderer over `gpu` that publishes `bridge`'s scene to it.
    #[must_use]
    pub fn new(gpu: G, bridge: Arc<SceneBridge>) -> Self {
        Self { gpu, bridge }
    }

    /// The scene bridge.
    #[must_use]
    pub fn bridge(&self) -> &Arc<SceneBridge> {
        &self.bridge
    }

    /// The backend.
    #[must_use]
    pub fn gpu(&self) -> &G {
        &self.gpu
    }

    /// Reads shown-frame times without waiting for or requesting another frame.
    #[must_use]
    #[cfg(all(feature = "metal", target_os = "macos"))]
    pub fn presented_times_reader(&self) -> Option<crate::PresentedTimesReader> {
        self.gpu.presented_times_reader()
    }
}

impl<G: Gpu2d> Presenter for Renderer<G> {
    type Handles = G::Handles;

    fn start(
        &mut self,
        handles: Self::Handles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
    ) -> Result<(), String> {
        let sink = self.gpu.new_sink();
        self.bridge
            .attach(desc, pixel_size, Arc::clone(&sink))
            .map_err(|refused| refused.to_string())?;
        let started = self.gpu.start(handles, desc, pixel_size, sink);
        if started.is_err() {
            self.bridge.detach();
        }
        started
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.gpu.set_clear(rgb);
    }

    fn set_size(&mut self, points: (u32, u32), pixels: PixelSize) {
        self.bridge.set_metrics(points, pixels);
        self.gpu.set_size(points, pixels);
    }

    fn request_redraw(&mut self) {
        self.gpu.request_redraw();
    }

    fn set_draw_every_refresh(&mut self, enabled: bool) {
        self.gpu.set_draw_every_refresh(enabled);
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.gpu.take_report()
    }

    fn request_stop(&mut self) {
        self.bridge.detach();
        self.gpu.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.gpu.poll_end(within)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeGpu(Arc<Mutex<Vec<bool>>>);

    impl Gpu2d for FakeGpu {
        type Handles = ();

        fn new_sink(&mut self) -> Arc<dyn QuadSink> {
            unreachable!("the forwarding test does not start a renderer")
        }

        fn start(
            &mut self,
            _handles: (),
            _: &WindowDesc,
            _: PixelSize,
            _: Arc<dyn QuadSink>,
        ) -> Result<(), String> {
            unreachable!("the forwarding test does not start a renderer")
        }

        fn set_clear(&mut self, _: [f32; 3]) {}

        fn set_pixel_size(&mut self, _: u32, _: u32) {}

        fn request_redraw(&mut self) {}

        fn set_draw_every_refresh(&mut self, enabled: bool) {
            self.0.lock().expect("draw settings").push(enabled);
        }

        fn take_report(&mut self) -> Option<PresenterReport> {
            None
        }

        fn request_stop(&mut self) {}

        fn poll_end(&mut self, _: Duration) -> Option<CleanupResult> {
            Some(CleanupResult::Proven)
        }
    }

    #[test]
    fn renderer_forwards_continuous_drawing_to_its_backend() {
        let settings = Arc::new(Mutex::new(Vec::new()));
        let mut renderer = Renderer::new(
            FakeGpu(Arc::clone(&settings)),
            SceneBridge::new(crate::Filter::Nearest),
        );
        Presenter::set_draw_every_refresh(&mut renderer, true);
        Presenter::set_draw_every_refresh(&mut renderer, false);
        assert_eq!(*settings.lock().expect("draw settings"), [true, false]);
    }
}
