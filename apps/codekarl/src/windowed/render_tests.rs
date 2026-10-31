//! The windowed run with the scene bridge: a real UI VM running `render.quad` and the renderer over fake
//! backends. A write after the end is discarded and counted. (The same script giving the same snapshots to a
//! Metal-shaped backend and to the Vulkan path is `vulkan::quad_tests`.)

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ck_render_2d::{Gpu2d, QuadSink, Renderer, Snapshot};
use ck_window::{CleanupResult, PixelSize, PresenterReport, WindowDesc};

use super::*;
use crate::fakes::{
    BackendScript, FakeBackend, FakeGpu, FakePresenter, PresenterScript, vector_recorder,
};
use crate::testing::{Dir, Extras, Gate, Harness, WINDOW, png};

/// What a snapshot comes to, for comparing runs: generation, whether a quad was requested, the placed quad's
/// destination and source in whole pixels and image pixels.
pub(crate) type Shape = (u64, bool, Option<([i32; 4], [f32; 4])>);

pub(crate) fn shape(snapshot: &Snapshot) -> Shape {
    (
        snapshot.generation,
        snapshot.requested,
        snapshot.quad.as_ref().map(|placed| {
            let (d, s) = (placed.destination, placed.source);
            (
                [d.left, d.top, d.right, d.bottom],
                [s.left, s.top, s.right, s.bottom],
            )
        }),
    )
}

/// Keeps every snapshot it is given, then hands it to the backend's own sink.
pub(crate) struct Tee {
    inner: Arc<dyn QuadSink>,
    seen: Arc<Mutex<Vec<Shape>>>,
}

impl QuadSink for Tee {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(shape(&snapshot));
        self.inner.publish(snapshot)
    }
}

/// A backend wrapped to record what its sink is given.
pub(crate) struct Recording<G> {
    pub(crate) inner: G,
    pub(crate) seen: Arc<Mutex<Vec<Shape>>>,
}

impl<G: Gpu2d> Gpu2d for Recording<G> {
    type Handles = G::Handles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        Arc::new(Tee {
            inner: self.inner.new_sink(),
            seen: Arc::clone(&self.seen),
        })
    }

    fn start(
        &mut self,
        handles: G::Handles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        // The two backends here ignore the sink they are started with (their own is the one inside `Tee`).
        self.inner.start(handles, desc, pixel_size, sink)
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.inner.set_clear(rgb);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.inner.set_pixel_size(width, height);
    }

    fn request_redraw(&mut self) {
        self.inner.request_redraw();
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.inner.take_report()
    }

    fn request_stop(&mut self) {
        self.inner.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.inner.poll_end(within)
    }
}

/// An assets directory with one 8 x 4 image, `small.png`.
pub(crate) fn assets() -> Dir {
    let dir = Dir::new();
    fs::write(dir.0.join("small.png"), png(8, 4, &[255; 8 * 4 * 4])).expect("an image");
    dir
}

pub(crate) const SCRIPT: &str =
    "render.quad{image='small.png', x=10, y=5}\nwindow.on_opened(function() window.close() end)";

/// Runs the script through `run_window` with `make(harness)` as the presenter and returns the snapshots the
/// bridge published to it, in order.
pub(crate) fn snapshots_of<P>(
    make: impl FnOnce(&Harness, Arc<Mutex<Vec<Shape>>>) -> P,
) -> (Vec<Shape>, RunEnd)
where
    P: Presenter<Handles = ()>,
{
    let dir = assets();
    let harness = Harness::new(
        &format!("{WINDOW}\n{SCRIPT}"),
        Extras {
            assets: Some(dir.0.clone()),
            ..Extras::default()
        },
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let presenter = make(&harness, Arc::clone(&seen));
    let latch = ResourceLatch::new();
    let (record, _events) = vector_recorder();
    let end = run_window(
        &harness.env(),
        || Ok(FakeBackend::new(BackendScript::default(), record)),
        presenter,
        &latch,
        DRAIN,
    );
    drop(dir);
    let shapes = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    (shapes, end)
}

#[test]
fn a_write_after_the_end_is_discarded_and_counted() {
    let dir = assets();
    let gate = Gate::new();
    let opened = Arc::new(AtomicBool::new(false));
    let hold: ck_lua::HostFunction = {
        let (gate, opened) = (Arc::clone(&gate), Arc::clone(&opened));
        Arc::new(move |_| {
            opened.store(true, Ordering::SeqCst);
            gate.wait();
            Ok(Vec::new())
        })
    };
    let harness = Harness::new(
        &format!(
            "{WINDOW}\nrender.quad{{image='small.png'}}\n\
             window.on_opened(function()\n  window.close()\n  window.hold()\n  render.quad{{image='small.png', x=1}}\nend)"
        ),
        Extras {
            functions: vec![("hold", hold)],
            assets: Some(dir.0.clone()),
            ..Extras::default()
        },
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let presenter = Renderer::new(
        Recording {
            inner: FakeGpu::default(),
            seen: Arc::clone(&seen),
        },
        Arc::clone(&harness.bridge),
    );
    let latch = ResourceLatch::new();
    let (record, _events) = vector_recorder();
    // A short drain: the callback is still held when the run returns.
    let end = run_window(
        &harness.env(),
        || Ok(FakeBackend::new(BackendScript::default(), record)),
        presenter,
        &latch,
        Duration::from_millis(50),
    );
    assert_eq!(end, RunEnd::Finished);
    assert!(opened.load(Ordering::SeqCst));
    let published = seen.lock().unwrap_or_else(PoisonError::into_inner).len();
    // Now the late write: the renderer let go at the end, so it changes nothing and publishes nothing.
    gate.open();
    let give_up = Instant::now() + Duration::from_secs(5);
    while harness.bridge.counts().accepted < 2 {
        assert!(Instant::now() < give_up, "the late write never arrived");
        std::thread::sleep(Duration::from_millis(1));
    }
    let counts = harness.bridge.counts();
    assert_eq!((counts.accepted, counts.discarded), (2, 1), "{counts:?}");
    assert_eq!(
        seen.lock().unwrap_or_else(PoisonError::into_inner).len(),
        published,
        "nothing was published after the end"
    );
}

/// The shipped demonstration script, run through `run_window` over the fake native backend and a Metal-shaped
/// renderer with the committed placeholder image, closes on a queued Escape with a clean end: the demo accepts
/// Escape (the real-window case declines it, since an owner may be typing; here it is the fake backend's).
#[test]
fn the_demo_script_closes_on_escape_with_a_clean_run_end_and_places_its_quad() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script =
        fs::read_to_string(root.join("../../scripts/ui/demo_quad.lua")).expect("the demo script");
    let assets = root.join("../../assets");
    let harness = Harness::new(
        &script,
        Extras {
            assets: Some(assets),
            ..Extras::default()
        },
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let presenter = Renderer::new(
        Recording {
            inner: FakeGpu::default(),
            seen: Arc::clone(&seen),
        },
        Arc::clone(&harness.bridge),
    );
    let latch = ResourceLatch::new();
    let (record, _events) = vector_recorder();
    let end = run_window(
        &harness.env(),
        || {
            let mut backend = FakeBackend::new(BackendScript::default(), record);
            let queue = Arc::clone(&backend.pending);
            backend.on_pump = Some(Box::new(move |pump| {
                if pump == 3 {
                    queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend([
                            NativeEvent::Key {
                                key: ck_window::Key::Escape,
                                down: true,
                                repeat: false,
                            },
                            NativeEvent::Key {
                                key: ck_window::Key::Escape,
                                down: false,
                                repeat: false,
                            },
                        ]);
                }
            }));
            Ok(backend)
        },
        presenter,
        &latch,
        DRAIN,
    );
    assert_eq!(end, RunEnd::Finished, "Escape closed the demo cleanly");
    assert!(!latch.is_set() && !harness.notes.unproven.get());
    let shapes = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let (_, requested, placed) = &shapes[0];
    assert!(*requested, "the demo's quad is in the first snapshot");
    // 256 x 256 points at (272, 172), at the fake window's scale of 2.
    assert_eq!(
        placed.as_ref().map(|(destination, _)| *destination),
        Some([544, 344, 1056, 856])
    );
}

/// The bridge ends when the loop returns, **before** the late-callback drain: a write made by a callback that is
/// still running while `run_window` waits for it is discarded and counted. The gate is opened by a thread that
/// waits for the bridge to be `Ended`, so if the end only came after the drain, the callback would never be
/// released in time. The presenter is not a renderer, so nothing but the application ends the bridge here.
#[test]
fn a_write_during_the_late_drain_is_discarded_and_counted() {
    let dir = assets();
    let gate = Gate::new();
    let hold: ck_lua::HostFunction = {
        let gate = Arc::clone(&gate);
        Arc::new(move |_| {
            gate.wait();
            Ok(Vec::new())
        })
    };
    let harness = Harness::new(
        &format!(
            "{WINDOW}\nwindow.on_opened(function()\n  window.close()\n  window.hold()\n  render.quad{{image='small.png'}}\nend)"
        ),
        Extras {
            functions: vec![("hold", hold)],
            assets: Some(dir.0.clone()),
            ..Extras::default()
        },
    );
    let opener = {
        let (gate, bridge) = (Arc::clone(&gate), Arc::clone(&harness.bridge));
        std::thread::spawn(move || {
            let give_up = Instant::now() + Duration::from_secs(10);
            while bridge.phase() != ck_render_2d::Phase::Ended && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(1));
            }
            gate.open();
        })
    };
    // No renderer: nothing but the application's own end of the bridge can end it here.
    let (record, _events) = vector_recorder();
    let presenter = FakePresenter::new(
        Arc::clone(&harness.notifier),
        PresenterScript::default(),
        Arc::clone(&record),
    );
    let latch = ResourceLatch::new();
    let end = run_window(
        &harness.env(),
        || Ok(FakeBackend::new(BackendScript::default(), record)),
        presenter,
        &latch,
        Duration::from_secs(8),
    );
    let _ = opener.join();
    assert_eq!(
        end,
        RunEnd::Finished,
        "the callback finished within the drain"
    );
    let counts = harness.bridge.counts();
    assert_eq!((counts.accepted, counts.discarded), (1, 1), "{counts:?}");
}
