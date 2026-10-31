//! The Metal backend's refusal path, delivered through a sleeping `ck_window::run` loop.
//!
//! Placement no longer hands the GPU crate a quad it would refuse (CK-92 r3), so these tests force the refusal
//! through the sink's test-only conversion seam. The adapter below is the backend's own sink, refusal record and
//! report, with fake handles instead of a window and a layer: nothing here opens a window or touches a GPU.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ck_gpu_metal::MetalScene;
use ck_window::{CleanupResult, LoopEnd, PixelSize, PresenterReport, WindowDesc};

use super::idle::{BOUND, IdleLoop};
use super::support::{image, quad_at};
use crate::metal::{Converted, ImagePixels, SinkSide};
use crate::{Filter, Gpu2d, QuadSink, SceneBridge, Snapshot};

/// A conversion that refuses every quad, as the GPU crate would an empty source span, and passes "no quad".
fn refuse(snapshot: &Snapshot, _reuse: Option<Arc<ImagePixels>>) -> Result<Converted, String> {
    if snapshot.quad.is_some() {
        Err("the Metal backend refused a quad: forced by the test".to_owned())
    } else {
        Ok((MetalScene::new(snapshot.generation, None), None))
    }
}

/// `MetalBackend`'s own sink side (`new_sink`, the recorded refusals, the wake, the report) over fake handles, with
/// only the conversion forced.
struct Refusing {
    side: SinkSide,
    /// Every failure the loop took, in order.
    reports: Arc<Mutex<Vec<String>>>,
}

impl Gpu2d for Refusing {
    type Handles = u32;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.side.new_sink()
    }

    fn start(
        &mut self,
        _handles: u32,
        _desc: &WindowDesc,
        _pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.side.accept(&sink).map(|_| ())
    }

    fn set_clear(&mut self, _rgb: [f32; 3]) {}

    fn set_pixel_size(&mut self, _width: u32, _height: u32) {}

    fn request_redraw(&mut self) {}

    fn take_report(&mut self) -> Option<PresenterReport> {
        let report = self.side.report();
        if let Some(PresenterReport::Failed { message }) = &report {
            self.reports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message.clone());
        }
        report
    }

    fn request_stop(&mut self) {}

    fn poll_end(&mut self, _within: Duration) -> Option<CleanupResult> {
        Some(CleanupResult::Proven)
    }
}

fn backend(wake: Arc<dyn Fn() + Send + Sync>, reports: Arc<Mutex<Vec<String>>>) -> Refusing {
    Refusing {
        side: SinkSide::with_converter(wake, refuse),
        reports,
    }
}

fn assert_refusal_ended_the_run(end: Option<LoopEnd>, reports: &Mutex<Vec<String>>) {
    match end {
        Some(LoopEnd::PresenterFailed { message }) => {
            assert!(message.contains("refused a quad"), "{message}");
        }
        other => panic!("the loop should have ended with the presenter's failure: {other:?}"),
    }
    let reports = reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(reports.len(), 1, "one failure per run: {reports:?}");
}

#[test]
fn a_refusal_reaches_a_sleeping_loop_with_no_input_event_redraw_or_answer() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&reports);
    let idle = IdleLoop::start(&bridge, move |wake| backend(wake, kept), None);
    idle.native.wait_sleeping(1);
    writer.set_quad(quad_at(&image(10, 10), 10.0, 10.0, 10.0, 10.0));
    assert_refusal_ended_the_run(idle.end_within(BOUND), &reports);
}

#[test]
fn a_refusal_published_just_before_the_wait_arms_is_not_lost() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let picture = image(10, 10);
    let mut held = Some(writer);
    // The first sleeping pump publishes at its very entry, before it looks at the native wake: the wake the
    // sink makes is already queued (sticky) when the wait would arm.
    let hook: Box<dyn FnMut() + Send> = Box::new(move || {
        if let Some(writer) = held.take() {
            writer.set_quad(quad_at(&picture, 10.0, 10.0, 10.0, 10.0));
        }
    });
    let reports = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&reports);
    let idle = IdleLoop::start(&bridge, move |wake| backend(wake, kept), Some(hook));
    assert_refusal_ended_the_run(idle.end_within(BOUND), &reports);
}
